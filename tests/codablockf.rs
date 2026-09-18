//! Codablock F end-to-end tests.
//!
//! 1. An **independent reference vector** from zint 2.16.0 (`zint
//!    --barcode=CODABLOCKF --data=ABCDEFGHIJKL --dump`): the exact module rows of a
//!    documented symbol are reproduced from its Code 128 codeword grid, and the symbol
//!    decodes back to the original text.
//! 2. `encode -> decode -> re-encode` identity across sizes.
#![cfg(all(feature = "decode", feature = "encode", feature = "codablockf"))]

use anyd::codes::codablockf::{CodablockFDecoder, CodablockFEncoder, CodablockFMeta};
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

#[test]
fn zint_reference_abcdefghijkl() {
    // Full Code 128 codeword grid for "ABCDEFGHIJKL" (zint --verbose), rows=4 cols=9.
    #[rustfmt::skip]
    let grid: Vec<u8> = vec![
        103, 100, 66, 33, 34, 35, 36, 34, 106,
        103, 100, 11, 37, 38, 39, 40, 99, 106,
        103, 100, 12, 41, 42, 43, 44, 70, 106,
        103, 100, 13, 99, 100, 30, 44,  1, 106,
    ];
    let meta = CodablockFMeta {
        rows: 4,
        columns: 9,
        codewords: grid,
    };
    let symbol = Symbol::new(
        Symbology::CodablockF,
        Vec::new(),
        SymbolMeta::CodablockF(meta),
    );
    let Encoding::Matrix(m) = CodablockFEncoder::new().encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!(m.width(), 101);
    assert_eq!(m.height(), 4);

    let expected = [
        "D0 97 BA 43 51 88 B1 11 AC 44 58 C7 58",
        "D0 97 BB 12 46 88 C5 A2 31 45 DE C7 58",
        "D0 97 BA CE 62 2B 71 63 A3 75 84 C7 58",
        "D0 97 BA 6E 5D EB DD B6 23 76 6C C7 58",
    ];
    for (row, hex) in expected.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 101).iter().enumerate() {
            assert_eq!(m.get(x, row), b, "row {row} col {x} differs from zint");
        }
    }

    let decoded = CodablockFDecoder::new()
        .decode(&Encoding::Matrix(m))
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("ABCDEFGHIJKL"));
}

fn assert_lossless(data: &[u8]) {
    let enc = CodablockFEncoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = CodablockFDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), data, "payload differs");
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_various() {
    assert_lossless(b"AB");
    assert_lossless(b"ABCDEFGHIJKL");
    assert_lossless(b"Hello, World!");
    assert_lossless(b"1234567890");
    assert_lossless(b"Codablock-F mixes UPPER, lower, 0-9 and symbols !@#$%^&*()");
    assert_lossless(b"\x01\x02\x1fcontrol and text");
    assert_lossless(&[b'Z'; 200]);
}

#[test]
fn rejects_non_codablockf_symbol() {
    let enc = CodablockFEncoder::new();
    let bogus = Symbol::new(Symbology::Code128, Vec::new(), SymbolMeta::Generic);
    assert!(enc.encode(&bogus).is_err());
}

/// Extended ASCII via FNC4 (zint 2.16.0, `zint -b CODABLOCKF --binary --esc -d
/// 'A\xE9B \xE0\xE1\xE2\xE3\xE4\xE5x\xE6' --dump`): single-FNC4 shifts and a
/// double-FNC4 latched run. K1/K2 are computed over the extended bytes, so the symbol
/// only validates when FNC4 is honoured.
#[test]
fn zint_extended_ascii_fnc4() {
    let dump = [
        "D0 97 BA 13 51 8B DD 0D 22 C4 26 C7 58",
        "D0 97 BB 12 6C CB DD 43 2E F7 A8 C7 58",
        "D0 97 BA CE 5E E9 61 7B A4 34 46 C7 58",
        "D0 97 BA 6E 5E E8 59 7B A1 37 68 C7 58",
        "D0 97 BA 67 5E EB 21 E4 AE F6 F6 C7 58",
        "D0 97 BA E6 5E EB 09 84 A2 C6 EE C7 58",
    ];
    let width = 9 * 11 + 2;
    let mut m = anyd::output::BitMatrix::new(width, dump.len(), 10);
    for (y, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, width).iter().enumerate() {
            m.set(x, y, b);
        }
    }
    let encoding = Encoding::Matrix(m);
    let decoded = CodablockFDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.payload_bytes(),
        b"A\xE9B \xE0\xE1\xE2\xE3\xE4\xE5x\xE6"
    );
    assert_eq!(CodablockFEncoder::new().encode(&decoded).unwrap(), encoding);
}
