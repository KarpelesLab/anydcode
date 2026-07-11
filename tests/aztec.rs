//! End-to-end Aztec round-trip tests: encode → decode → re-encode must be identical,
//! across encodation modes and both compact and full-range sizes, and must survive
//! correctable errors.

use anyd::codes::aztec::{AztecDecoder, AztecEncoder};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

/// Assert a payload decodes to the same bytes/meta and re-encodes identically.
fn assert_lossless(payload: &[u8]) {
    let enc = AztecEncoder::new();
    let symbol = enc.build(vec![Segment::byte(payload.to_vec())]).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = AztecDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.payload_bytes(),
        payload,
        "payload differs after decode"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs after decode");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn upper_and_digit_modes() {
    assert_lossless(b"AZTEC CODE 2026");
}

#[test]
fn lower_mixed_punct_modes() {
    assert_lossless(b"Hello, World! (mixed & punct: a-z/0-9)");
}

#[test]
fn binary_mode_high_bytes() {
    assert_lossless(&[0x00, 0x01, 0x80, 0xff, 0xfe, b'A', 0x0e, 0x1a, 0x7f]);
}

#[test]
fn utf8_payload() {
    assert_lossless("héllo wörld — 日本語".as_bytes());
}

#[test]
fn all_control_and_symbols() {
    assert_lossless(b"a\tb\nc\rd @\\^_`|~ #$%&*+=<>?[]{}");
}

#[test]
fn compact_sizes_scale_with_length() {
    // Increasing payloads walk through the compact layer counts.
    let mut compact_layers_seen = std::collections::BTreeSet::new();
    for n in [1usize, 10, 30, 60, 120, 200] {
        let payload = vec![b'A'; n];
        let enc = AztecEncoder::new();
        let symbol = enc.build(vec![Segment::byte(payload.clone())]).unwrap();
        if let SymbolMeta::Aztec(m) = &symbol.meta
            && m.compact
        {
            compact_layers_seen.insert(m.layers);
        }
        let encoding = enc.encode(&symbol).unwrap();
        let decoded = AztecDecoder::new().decode(&encoding).unwrap();
        assert_eq!(decoded.payload_bytes(), payload);
        assert_eq!(enc.encode(&decoded).unwrap(), encoding);
    }
    // We should have exercised more than one compact size.
    assert!(
        compact_layers_seen.len() >= 2,
        "expected multiple compact sizes"
    );
}

#[test]
fn full_range_symbol() {
    // A payload too large for compact (max 4 layers) forces a full-range symbol.
    let payload: Vec<u8> = (0..600u16).map(|i| b'A' + (i % 26) as u8).collect();
    let enc = AztecEncoder::new();
    let symbol = enc.build(vec![Segment::byte(payload.clone())]).unwrap();
    match &symbol.meta {
        SymbolMeta::Aztec(m) => assert!(!m.compact, "expected a full-range symbol"),
        _ => panic!("expected Aztec meta"),
    }
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = AztecDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), payload);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn survives_correctable_errors() {
    // Corrupt a few data modules and confirm Reed–Solomon still recovers the payload.
    let enc = AztecEncoder::new();
    let symbol = enc.build_text("RESILIENT AZTEC PAYLOAD").unwrap();
    let Encoding::Matrix(mut matrix) = enc.encode(&symbol).unwrap() else {
        panic!("expected a matrix");
    };
    // Flip a short run of modules away from the bullseye.
    for k in 0..4 {
        let x = matrix.width() - 2 - k;
        let y = 1 + k;
        let v = matrix.get(x, y);
        matrix.set(x, y, !v);
    }
    let decoded = AztecDecoder::new()
        .decode(&Encoding::Matrix(matrix))
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("RESILIENT AZTEC PAYLOAD"));
}

#[test]
fn text_convenience_roundtrip() {
    let enc = AztecEncoder::new();
    let symbol = enc
        .build_text("https://github.com/KarpelesLab/anydcode")
        .unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = AztecDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("https://github.com/KarpelesLab/anydcode")
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}
