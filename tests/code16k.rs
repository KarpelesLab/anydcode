//! Code 16K end-to-end tests.
//!
//! Two kinds of check:
//! 1. An **independent reference vector** taken from zint 2.16.0 (`zint
//!    --barcode=CODE16K --data=ABCD1234 --dump`): the exact module rows of a documented
//!    symbol are reproduced from the corresponding symbol values.
//! 2. `encode -> decode -> re-encode` identity across sizes, plus payload recovery.
#![cfg(all(feature = "decode", feature = "encode", feature = "code16k"))]

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

/// Decode a zint `--dump` (one hex row per symbol row) as a Code 16K matrix.
fn decode_zint_dump(rows: &[&str]) -> Symbol {
    let mut m = anyd::output::BitMatrix::new(70, rows.len(), 10);
    for (y, hex) in rows.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 70).iter().enumerate() {
            m.set(x, y, b);
        }
    }
    Code16kDecoder::new().decode(&Encoding::Matrix(m)).unwrap()
}

/// Modes 5 and 6 start in Code Set C with an implied Shift B on the first one / two
/// data characters (EN 12323 Table 2). Reference symbols from zint 2.16.0
/// (`zint -b CODE16K -d A1234 --dump` and `-d AB1234`), which selects these modes for
/// one or two leading non-digits followed by digit pairs.
#[test]
fn zint_implied_shift_b_modes() {
    let decoded = decode_zint_dump(&["E5 76 6B 9D 31 BA 72 F6 34", "CD 2F 65 EC BD A1 13 5E 64"]);
    let SymbolMeta::Code16k(meta) = &decoded.meta else {
        panic!();
    };
    assert_eq!(meta.values[..4], [5, 33, 12, 34]);
    assert_eq!(decoded.text().as_deref(), Some("A1234"));

    let decoded = decode_zint_dump(&["E5 66 EB 9D D3 A6 37 4E 34", "CD 2F 65 EC BD BA 16 16 64"]);
    let SymbolMeta::Code16k(meta) = &decoded.meta else {
        panic!();
    };
    assert_eq!(meta.values[..5], [6, 33, 34, 12, 34]);
    assert_eq!(decoded.text().as_deref(), Some("AB1234"));
}

/// Symbol value 106 (reachable only as a modulo-107 check character) has its own
/// Code 16K pattern `211133`; it is not Code 128's Stop character cut to six elements.
/// Reference: zint 2.16.0 `zint -b CODE16K -d lnlmfghxvylucwb --dump`, whose second
/// check character is 106.
#[test]
fn zint_check_character_106() {
    let dump = [
        "E5 46 66 BC F5 9A F0 8A 34",
        "CD 4F 6C BD 9E 86 D0 B6 64",
        "D9 24 26 BD 86 BD 30 D6 4C",
        "85 6F 25 EC BD BB 32 8E F4",
    ];
    let decoded = decode_zint_dump(&dump);
    assert_eq!(decoded.text().as_deref(), Some("lnlmfghxvylucwb"));

    // Our own encoder picks the same symbol values here, so the output is identical.
    let enc = Code16kEncoder::new();
    let symbol = enc.build(b"lnlmfghxvylucwb").unwrap();
    assert_eq!(symbol.meta, decoded.meta);
    let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    for (row, hex) in dump.iter().enumerate() {
        for (x, &b) in bits_from_hex(hex, 70).iter().enumerate() {
            assert_eq!(m.get(x, row), b, "row {row} col {x} differs from zint");
        }
    }
}

/// Extended ASCII via FNC4 (zint 2.16.0, `--binary --esc`): a single FNC4 shifts one
/// character by 128 (`A\xE9B`), a double FNC4 latches a run of them and a single FNC4
/// inside the run exempts the plain `x` (`\xE0\xE1\xE2\xE3\xE4\xE5x\xE6`).
#[test]
fn zint_extended_ascii_fnc4() {
    let decoded = decode_zint_dump(&["E5 32 6B 9D 08 BC B7 4E 34", "CD 2F 65 EC BD 9D B4 2E 64"]);
    assert_eq!(decoded.payload_bytes(), b"A\xE9B");

    let decoded = decode_zint_dump(&[
        "E5 46 68 45 79 A1 16 9E 34",
        "CD 42 2D E5 08 BD 34 22 64",
        "D9 7B 28 45 37 86 D4 22 4C",
        "85 4F 65 EC BD B3 75 CE F4",
    ]);
    assert_eq!(decoded.payload_bytes(), b"\xE0\xE1\xE2\xE3\xE4\xE5x\xE6");
}
