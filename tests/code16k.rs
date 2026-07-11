//! Code 16K end-to-end tests.
//!
//! Two kinds of check:
//! 1. An **independent reference vector** taken from zint 2.16.0 (`zint
//!    --barcode=CODE16K --data=ABCD1234 --dump`): the exact module rows of a documented
//!    symbol are reproduced from the corresponding symbol values.
//! 2. `encode -> decode -> re-encode` identity across sizes, plus payload recovery.

use anyd::codes::code16k::{Code16kDecoder, Code16kEncoder, Code16kMeta};
use anyd::output::Encoding;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Expand a zint `--dump` hex row (MSB first) into `width` booleans.
fn bits_from_hex(hex: &str, width: usize) -> Vec<bool> {
    let bytes: Vec<u8> = hex
        .split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect();
    let mut out = Vec::with_capacity(width);
    for i in 0..width {
        let byte = bytes[i / 8];
        let bit = 7 - (i % 8);
        out.push((byte >> bit) & 1 == 1);
    }
    out
}

#[test]
fn zint_reference_abcd1234_modules() {
    // Symbol values for "ABCD1234" (zint --verbose): mode char + data, no checks.
    let meta = Code16kMeta {
        rows: 2,
        values: vec![1, 33, 34, 35, 36, 99, 12, 34],
    };
    let symbol = Symbol::new(Symbology::Code16k, Vec::new(), SymbolMeta::Code16k(meta));
    let Encoding::Matrix(m) = Code16kEncoder::new().encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!(m.width(), 70);
    assert_eq!(m.height(), 2);

    // zint 2.16.0 `--dump` of the same symbol.
    let expected = ["E5 32 6B 9D D3 BB 94 EE 34", "CD 44 29 8D D3 9D B3 AE 64"];
    for (row, hex) in expected.iter().enumerate() {
        let want = bits_from_hex(hex, 70);
        for (x, &b) in want.iter().enumerate() {
            assert_eq!(m.get(x, row), b, "row {row} col {x} differs from zint");
        }
    }

    // And the symbol decodes back to "ABCD1234".
    let decoded = Code16kDecoder::new().decode(&Encoding::Matrix(m)).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("ABCD1234"));
}

fn assert_lossless(data: &[u8]) {
    let enc = Code16kEncoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = Code16kDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    assert_eq!(decoded.payload_bytes(), data, "payload differs");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_various() {
    assert_lossless(b"A");
    assert_lossless(b"ABCD1234");
    assert_lossless(b"Hello, World!");
    assert_lossless(b"1234567890");
    assert_lossless(b"\x01\x02control\x1f mix ABC");
    // A payload spanning many rows.
    assert_lossless(&[b'X'; 60]);
}

#[test]
fn explicit_min_rows() {
    let enc = Code16kEncoder::new();
    let symbol = enc.build_rows(b"AB", Some(6)).unwrap();
    let SymbolMeta::Code16k(meta) = &symbol.meta else {
        panic!();
    };
    assert_eq!(meta.rows, 6);
    let encoding = enc.encode(&symbol).unwrap();
    let Encoding::Matrix(m) = &encoding else {
        panic!();
    };
    assert_eq!(m.height(), 6);
    let decoded = Code16kDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.meta, symbol.meta);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn rejects_non_code16k_symbol() {
    let enc = Code16kEncoder::new();
    let bogus = Symbol::new(Symbology::Code128, Vec::new(), SymbolMeta::Generic);
    assert!(enc.encode(&bogus).is_err());
}
