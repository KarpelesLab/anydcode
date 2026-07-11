//! Code 11 end-to-end tests: independent checksum reference vectors plus
//! encode → decode → re-encode identity across 0/1/2 check characters.

use anydcode::codes::code11::{Code11Decoder, Code11Encoder};
use anydcode::output::Encoding;
use anydcode::segment::Segment;
use anydcode::symbol::SymbolMeta;
use anydcode::traits::{Decode, Encode};

fn modules(enc: &Encoding) -> Vec<bool> {
    match enc {
        Encoding::Linear(p) => p.modules.clone(),
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

/// Reference vector: `012345` has one check character C = `2` (Wikipedia caption
/// "0123452"). Encoding `012345` with one check must equal encoding the literal
/// `0123452` with none.
#[test]
fn reference_vector_012345_check_is_2() {
    let enc = Code11Encoder::new();
    let with_check = enc.encode(&enc.build(b"012345", 1).unwrap()).unwrap();
    let literal = enc.encode(&enc.build(b"0123452", 0).unwrap()).unwrap();
    assert_eq!(modules(&with_check), modules(&literal));
}

/// Reference vector: `123-45` has C = `5`, K = `2` (Bar Code Island Code 11
/// specification). Two-check encoding must equal the literal `123-4552` with none.
#[test]
fn reference_vector_123_45_checks_are_5_2() {
    let enc = Code11Encoder::new();
    let with_checks = enc.encode(&enc.build(b"123-45", 2).unwrap()).unwrap();
    let literal = enc.encode(&enc.build(b"123-4552", 0).unwrap()).unwrap();
    assert_eq!(modules(&with_checks), modules(&literal));
}

fn assert_roundtrip(data: &[u8], check_count: u8) {
    let enc = Code11Encoder::new();
    let dec = Code11Decoder::new().with_check_count(check_count);

    let symbol = enc.build(data, check_count).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    match &decoded.meta {
        SymbolMeta::Code11(m) => assert_eq!(m.check_count, check_count),
        _ => panic!("wrong meta variant"),
    }

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_check_count_matrix() {
    for &count in &[0u8, 1, 2] {
        assert_roundtrip(b"012345", count);
        assert_roundtrip(b"123-45", count);
        assert_roundtrip(b"9-9-9-9", count);
        assert_roundtrip(b"0000000000001", count); // > 10 chars
        assert_roundtrip(b"5", count);
    }
}

#[test]
fn numeric_vs_alphanumeric_segment() {
    let enc = Code11Encoder::new();
    // All-digit payload is a numeric segment.
    let s = enc.build(b"12345", 1).unwrap();
    assert_eq!(s.segments, vec![Segment::numeric(b"12345".to_vec())]);
    // Payload containing a dash is alphanumeric.
    let s = enc.build(b"12-34", 1).unwrap();
    assert_eq!(s.segments, vec![Segment::alphanumeric(b"12-34".to_vec())]);
}

#[test]
fn rejects_non_code11_bytes() {
    let enc = Code11Encoder::new();
    assert!(enc.build(b"12A45", 1).is_err());
}

#[test]
fn corrupt_check_is_rejected() {
    let enc = Code11Encoder::new();
    let symbol = enc.build(b"12345", 0).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    // Demand a check the data was not built with; last digit rarely validates.
    let result = Code11Decoder::new().with_check_count(1).decode(&encoding);
    assert!(result.is_err());
}
