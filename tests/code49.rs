//! Code 49 end-to-end tests.
//!
//! 1. **Independent reference vectors** from zint 2.16.0 (`zint --barcode=CODE49
//!    --data=... --dump`): three documented symbols (alphanumeric, letters, and a
//!    numeric-mode symbol) are reproduced module-for-module from their code-character
//!    grids, exercising the even/odd encodation-pattern tables.
//! 2. `encode -> decode -> re-encode` identity, plus payload recovery.
#![cfg(all(feature = "decode", feature = "encode", feature = "code49"))]

use anyd::codes::code49::{Code49Decoder, Code49Encoder, Code49Meta};
use anyd::output::Encoding;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn bits_from_hex(hex: &str, width: usize) -> Vec<bool> {
    let bytes: Vec<u8> = hex
        .split_whitespace()
        .map(|b| u8::from_str_radix(b, 16).unwrap())
        .collect();
    (0..width)
        .map(|i| (bytes[i / 8] >> (7 - (i % 8))) & 1 == 1)
        .collect()
}

fn assert_render(grid: Vec<u8>, rows: usize, dump: &[&str]) {
    let meta = Code49Meta { rows, grid };
    let symbol = Symbol::new(Symbology::Code49, Vec::new(), SymbolMeta::Code49(meta));
    let Encoding::Matrix(m) = Code49Encoder::new().encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!(m.width(), 70);
    assert_eq!(m.height(), rows);
    for (row, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 70).iter().enumerate() {
            assert_eq!(m.get(x, row), b, "row {row} col {x} differs from zint");
        }
    }
}

#[test]
fn zint_reference_abcd1234() {
    assert_render(
        vec![10, 11, 12, 13, 1, 2, 3, 3, 4, 48, 26, 9, 17, 31, 0, 37],
        2,
        &["BF 6C A7 B0 BE 99 BB E6 BC", "AE 11 36 28 3B A3 A3 D8 BC"],
    );
    // ...and it decodes back.
    let meta = Code49Meta {
        rows: 2,
        grid: vec![10, 11, 12, 13, 1, 2, 3, 3, 4, 48, 26, 9, 17, 31, 0, 37],
    };
    let symbol = Symbol::new(Symbology::Code49, Vec::new(), SymbolMeta::Code49(meta));
    let encoding = Code49Encoder::new().encode(&symbol).unwrap();
    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("ABCD1234"));
}

#[test]
fn zint_reference_hello() {
    assert_render(
        vec![17, 14, 21, 21, 24, 48, 48, 46, 48, 48, 47, 47, 39, 19, 0, 3],
        2,
        &["A0 8C AE 23 BF 76 B4 DF BC", "B3 CB B7 26 20 C5 B0 49 BC"],
    );
}

#[test]
fn zint_reference_numeric() {
    // Uses Code 49 Numeric Encodation (mode value 2) — a render-only vector.
    assert_render(
        vec![5, 17, 9, 29, 22, 18, 48, 1, 48, 48, 43, 23, 8, 0, 2, 25],
        2,
        &["B0 DE A1 D3 A8 2F B5 8F 3C", "B3 CB B7 3C B2 8C 2E E2 3C"],
    );
}

fn assert_lossless(data: &[u8]) {
    let enc = Code49Encoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), data, "payload differs");
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_various() {
    assert_lossless(b"A");
    assert_lossless(b"ABCD1234");
    assert_lossless(b"HELLO WORLD");
    assert_lossless(b"CODE-49 $/+%.");
    assert_lossless(b"1234567890"); // digits encode individually (no numeric mode)
    assert_lossless(b"Mixed Case abc XYZ 42");
    assert_lossless(b"\x01\x02\x1f ctrl");
    assert_lossless(&[b'Q'; 40]);
}

#[test]
fn explicit_min_rows() {
    let enc = Code49Encoder::new();
    let symbol = enc.build_rows(b"AB", Some(5)).unwrap();
    let SymbolMeta::Code49(meta) = &symbol.meta else {
        panic!();
    };
    assert_eq!(meta.rows, 5);
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.meta, symbol.meta);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn rejects_non_code49_symbol() {
    let enc = Code49Encoder::new();
    let bogus = Symbol::new(Symbology::Code128, Vec::new(), SymbolMeta::Generic);
    assert!(enc.encode(&bogus).is_err());
}

/// Decode a zint `--dump` as a Code 49 matrix and check payload + re-encode identity.
fn assert_zint_decodes(dump: &[&str], payload: &[u8]) {
    let mut m = anyd::output::BitMatrix::new(70, dump.len(), 10);
    for (y, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 70).iter().enumerate() {
            m.set(x, y, b);
        }
    }
    let encoding = Encoding::Matrix(m);
    let decoded = Code49Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), payload);
    assert_eq!(Code49Encoder::new().encode(&decoded).unwrap(), encoding);
}

/// Numeric Encodation (toggled by codeword 48, or selected by starting mode 2): three
/// base-48 codewords carry five digits (or four, offset by 100000), two carry three
/// digits and one carries a single digit. Reference symbols from zint 2.16.0
/// (`zint -b CODE49 -d ... --dump`), which uses it for every run of five or more digits.
#[test]
fn zint_numeric_encodation_decodes() {
    // Starting mode 2; 5 + 5 + 1 digits.
    assert_zint_decodes(
        &["B0 DE A1 D3 A8 2F A3 B4 3C", "B3 CB B6 44 21 B3 A1 33 3C"],
        b"12345678901",
    );
    // 5 + 4 + 3 digits.
    assert_zint_decodes(
        &["B0 DE BD 67 2B F7 A1 68 3C", "B2 67 2B 3C 31 6E 30 98 BC"],
        b"123456789012",
    );
    // Numeric shift in and out between letters; 4 + 3 digits.
    assert_zint_decodes(
        &["B8 C9 28 36 38 88 AB CE 3C", "A6 C4 36 5F 27 CB BA EE 3C"],
        b"A1234567B",
    );
    // A numeric run, back to alphanumeric, then a short digit run left unshifted.
    assert_zint_decodes(
        &[
            "AF D0 A3 1B BD 23 22 5E 3C",
            "BA 38 BB D2 33 CB B6 47 BC",
            "B3 CB B2 D0 3B 16 28 33 3C",
        ],
        b"X12345Y678",
    );
}
