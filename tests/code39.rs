//! Code 39 end-to-end tests: independent reference vectors plus encode → decode →
//! re-encode identity across the option matrix (standard/full-ASCII, with/without a
//! mod-43 check character).

use anyd::codes::code39::{Code39Decoder, Code39Encoder};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

/// Turn a `1`/`0` string into a module vector (`true` = bar).
fn bits(s: &str) -> Vec<bool> {
    s.bytes().map(|b| b == b'1').collect()
}

fn modules(enc: &Encoding) -> &Vec<bool> {
    match enc {
        Encoding::Linear(p) => &p.modules,
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

/// Reference vector: `*A*` assembled from the published Code 39 patterns
/// (start/stop `*` = 100101101101, `A` = 110101001011) joined by narrow gaps.
/// Source: Bar Code Island Code 39 specification.
#[test]
fn reference_vector_star_a_star() {
    let enc = Code39Encoder::new();
    let symbol = enc.build(b"A", false, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let star = "100101101101";
    let a = "110101001011";
    let expected = bits(&format!("{star}0{a}0{star}"));
    assert_eq!(modules(&encoding), &expected);
}

/// Reference vector: the mod-43 check character of "ABC" is 33 = `X`
/// (10 + 11 + 12 = 33). Encoding "ABC" with a check must equal encoding the literal
/// "ABCX" without one.
#[test]
fn reference_vector_mod43_check_abc_is_x() {
    let enc = Code39Encoder::new();
    let with_check = enc
        .encode(&enc.build(b"ABC", false, true).unwrap())
        .unwrap();
    let literal = enc
        .encode(&enc.build(b"ABCX", false, false).unwrap())
        .unwrap();
    assert_eq!(modules(&with_check), modules(&literal));
}

/// Full assert: decode recovers the exact segments and meta, and the decoded symbol
/// re-encodes byte-for-byte.
fn assert_roundtrip(data: &[u8], full_ascii: bool, check_digit: bool) {
    let enc = Code39Encoder::new();
    let dec = Code39Decoder::new()
        .with_full_ascii(full_ascii)
        .with_check_digit(check_digit);

    let symbol = enc.build(data, full_ascii, check_digit).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    match &decoded.meta {
        SymbolMeta::Code39(m) => {
            assert_eq!(m.full_ascii, full_ascii);
            assert_eq!(m.check_digit, check_digit);
        }
        _ => panic!("wrong meta variant"),
    }

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_standard_option_matrix() {
    for &check in &[false, true] {
        assert_roundtrip(b"CODE39", false, check);
        assert_roundtrip(b"ABC-123 $/+%.", false, check);
        assert_roundtrip(b"0123456789", false, check);
        assert_roundtrip(b"", false, check);
    }
}

#[test]
fn roundtrip_full_ascii_option_matrix() {
    for &check in &[false, true] {
        assert_roundtrip(b"Hello, World!", true, check);
        assert_roundtrip(b"abc-XYZ_123", true, check);
        // Base characters via full-ASCII still round-trip (as a byte segment).
        assert_roundtrip(b"ABC 123", true, check);
    }
}

#[test]
fn roundtrip_all_ascii_bytes() {
    // Every ASCII byte through the full-ASCII shift machinery, with a check character.
    let all: Vec<u8> = (0u8..128).collect();
    assert_roundtrip(&all, true, true);
}

#[test]
fn decoded_payload_is_recoverable_text() {
    let enc = Code39Encoder::new();
    let symbol = enc.build(b"Hello!", true, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = Code39Decoder::new()
        .with_full_ascii(true)
        .decode(&encoding)
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("Hello!"));
    assert_eq!(decoded.segments, vec![Segment::byte(b"Hello!".to_vec())]);
}

#[test]
fn corrupt_check_is_rejected() {
    let enc = Code39Encoder::new();
    // Build without a check, then decode demanding one: the last data char is treated
    // as a check and will almost never validate.
    let symbol = enc.build(b"CODE39", false, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let result = Code39Decoder::new()
        .with_check_digit(true)
        .decode(&encoding);
    assert!(result.is_err());
}
