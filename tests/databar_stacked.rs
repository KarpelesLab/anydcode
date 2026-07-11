//! GS1 DataBar stacked-variant end-to-end tests.
//!
//! Two kinds of check:
//! 1. **Independent reference vectors** — the exact module bitmap for a known input,
//!    rendered with BWIPP (`postscriptbarcode`, via the `bwip-js` port) using the
//!    `databarstacked` / `databarstackedomni` / `databarexpandedstacked` encoders.
//!    BWIPP is an implementation independent of this crate's zint-derived algorithm.
//!    Each logical module-row (data or separator) is one text line; `1` = dark,
//!    `0` = light, left to right. The two implementations agree module-for-module.
//! 2. **Lossless round-trip** — encode -> decode -> re-encode reproduces the exact
//!    segments, metadata and module matrix.
//!
//! GTIN payloads are the 13-digit body (BWIPP is fed the `(01)` element string that
//! reduces to the same value). Expanded Stacked payloads are supplied in *reduced*
//! form (AI digits + values concatenated; FNC1 is byte `0x1D`), matching the linear
//! Expanded convention.

use anyd::codes::databar::{DataBarDecoder, DataBarEncoder, DataBarMeta, DataBarVariant};
use anyd::output::Encoding;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Render a matrix encoding to newline-separated `1`/`0` module rows.
fn matrix_rows(encoding: &Encoding) -> Vec<String> {
    let Encoding::Matrix(m) = encoding else {
        panic!("expected a matrix encoding");
    };
    (0..m.height())
        .map(|y| {
            (0..m.width())
                .map(|x| if m.get(x, y) { '1' } else { '0' })
                .collect()
        })
        .collect()
}

fn assert_bitmap(encoding: &Encoding, expected: &[&str], label: &str) {
    let rows = matrix_rows(encoding);
    assert_eq!(rows.len(), expected.len(), "row count mismatch for {label}");
    for (i, (got, want)) in rows.iter().zip(expected).enumerate() {
        assert_eq!(got, want, "row {i} mismatch for {label}");
    }
}

// ---- Independent reference vectors: DataBar Stacked (BWIPP `databarstacked`) ----

#[test]
fn stacked_reference_vectors() {
    let enc = DataBarEncoder::new();
    let cases: &[(&[u8], &[&str])] = &[
        (
            b"0101234567890",
            &[
                "01010010000000100100011111000001010101000000001010",
                "00000101101101011010100010111010101010110101110000",
                "10101000010011000101111100000111011100001010000101",
            ],
        ),
        (
            b"2001234567890",
            &[
                "01010001110100000100111111100001010011011011111010",
                "00001110101011011010010101011010101001001001010000",
                "10110000010010100101100000000111000110110110001101",
            ],
        ),
    ];
    for (gtin, expected) in cases {
        let symbol = enc.build_stacked(gtin).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        assert_bitmap(&encoding, expected, &format!("stacked {gtin:?}"));
    }
}

// ---- Reference vectors: Stacked Omnidirectional (BWIPP `databarstackedomni`) ----

#[test]
fn stacked_omni_reference_vectors() {
    let enc = DataBarEncoder::new();
    let cases: &[(&[u8], &[&str])] = &[
        (
            b"0101234567890",
            &[
                "01010010000000100100011111000001010101000000001010",
                "00001101111111011010100000101010101010111111110000",
                "00000101010101010101010101010101010101010101010000",
                "00000111101100111010000010101000100011110101110000",
                "10101000010011000101111100000111011100001010000101",
            ],
        ),
        // GTIN whose right finder value is 3, exercising the ISO/IEC 24724:2011
        // 5.3.2.2 "finder value 3" separator special case.
        (
            b"0000000000000",
            &[
                "01010100100000000100011111111001011111110010101010",
                "00001011011111111010100000000100100000001101010000",
                "00000101010101010101010101010101010101010101010000",
                "00000101001111111010000000000100100000000100100000",
                "10101010110000000101111111110111011111111011010101",
            ],
        ),
    ];
    for (gtin, expected) in cases {
        let symbol = enc.build_stacked_omni(gtin).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        assert_bitmap(&encoding, expected, &format!("stacked-omni {gtin:?}"));
    }
}

// ---- Reference vector: Expanded Stacked (BWIPP `databarexpandedstacked`) ----

#[test]
fn expanded_stacked_reference_vector() {
    let enc = DataBarEncoder::new();
    // Reduced form of `(01)09501101530003(17)140704(10)AB-123`, 2 column pairs/row.
    let reduced = b"01095011015300031714070410AB-123";
    let expected: &[&str] = &[
        "010101111100001001101111111100001011100001110110010100000011011010001011111000000110011010000001001101",
        "000010000011110110010000000010100100011110001001101011111100100101110100000101010001100101111110110000",
        "000001010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010000",
        "000011011001001111100001000000001011011111000110110110110000111110101001010000001010011101110100010000",
        "101000100110110000011100111111110100100000111001001001001111000001010100001111110001100010001011100010",
        "000011011001001111100001000000001011011111000110110110110000111110101001010000001010011101110100010000",
        "000001010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010000",
        "000010111101011110010100101010100100111100110010111001001100011111010100000000010000000000000000000000",
        "010001000010100001100011000000001011000011001101000110110011100000101011111111100110100000000000000000",
    ];
    let symbol = enc.build_expanded_stacked(reduced, 2).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    assert_bitmap(&encoding, expected, "expanded-stacked cols=2");
}

#[test]
fn expanded_stacked_reference_vector_cols4() {
    // Same payload at 4 column pairs/row: the last row is a right-to-left partial
    // row whose final codeblock is a 13-element (check/data + finder) block — the
    // most intricate Expanded Stacked layout case. BWIPP `databarexpandedstacked`
    // with `segments=8`.
    let enc = DataBarEncoder::new();
    let reduced = b"01095011015300031714070410AB-123";
    let expected: &[&str] = &[
        "01010111110000100110111111110000101110000111011001010000001101101000101111100000011001101000000100110001110100010001100011111100001010100000111100100100100111000001001011111111001110000011011001000101",
        "00001000001111011001000000001010010001111000100110101111110010010111010000010101000110010111111011001110001011101110010100000010100101011111000011011011011000111110110100000000100001111100100110110000",
        "00000101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010101010000",
        "00000100000000010101111100011001001110100110011110010010101010010100111101011110100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
        "10110011111111101010000011100110110001011001100001101000000001100011000010100001000100000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
    ];
    let symbol = enc.build_expanded_stacked(reduced, 4).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    assert_bitmap(&encoding, expected, "expanded-stacked cols=4");
}

// ---- Lossless round-trips ----

fn assert_roundtrip_gtin(build: impl Fn(&DataBarEncoder, &[u8]) -> anyd::Symbol, gtin: &[u8]) {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let symbol = build(&enc, gtin);
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(decoded.symbology, symbol.symbology, "symbology ({gtin:?})");
    assert_eq!(decoded.segments, symbol.segments, "segments ({gtin:?})");
    assert_eq!(decoded.meta, symbol.meta, "meta ({gtin:?})");
    assert_eq!(
        enc.encode(&decoded).unwrap(),
        encoding,
        "re-encode not identical ({gtin:?})"
    );
}

#[test]
fn stacked_roundtrip() {
    let gtins: &[&[u8]] = &[
        b"0101234567890",
        b"2001234567890",
        b"0000000000000",
        b"9999999999999",
        b"1234567890123",
        b"0095011015300",
    ];
    for gtin in gtins {
        assert_roundtrip_gtin(|e, g| e.build_stacked(g).unwrap(), gtin);
        assert_roundtrip_gtin(|e, g| e.build_stacked_omni(g).unwrap(), gtin);
    }
}

fn assert_roundtrip_expstk(reduced: &[u8], cols: usize) {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let symbol = enc.build_expanded_stacked(reduced, cols).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(
        decoded.symbology,
        Symbology::DataBarExpandedStacked,
        "symbology ({reduced:?} cols={cols})"
    );
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments ({reduced:?} cols={cols})"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta ({reduced:?} cols={cols})");
    assert_eq!(
        enc.encode(&decoded).unwrap(),
        encoding,
        "re-encode not identical ({reduced:?} cols={cols})"
    );
}

#[test]
fn expanded_stacked_roundtrip() {
    let payloads: &[&[u8]] = &[
        b"1012A",
        b"01095011015300031714070410AB-123",
        b"25595011015340010123456789",
        b"2559501101534001\x1d171605313902050",
    ];
    for reduced in payloads {
        for cols in 1..=4usize {
            assert_roundtrip_expstk(reduced, cols);
        }
    }
}

// ---- Metadata surface ----

#[test]
fn expanded_stacked_meta_records_columns() {
    let enc = DataBarEncoder::new();
    // A payload with at least 3 column pairs so 3 columns/row is realised exactly.
    let symbol = enc
        .build_expanded_stacked(b"01095011015300031714070410AB-123", 3)
        .unwrap();
    assert_eq!(
        symbol.meta,
        SymbolMeta::DataBar(DataBarMeta::expanded_stacked(3))
    );
    let SymbolMeta::DataBar(meta) = symbol.meta else {
        panic!("expected DataBar meta");
    };
    assert_eq!(meta.variant, DataBarVariant::ExpandedStacked);
    assert_eq!(meta.columns_per_row, Some(3));
}

#[test]
fn stacked_meta_has_no_columns() {
    let enc = DataBarEncoder::new();
    let symbol = enc.build_stacked(b"0101234567890").unwrap();
    let SymbolMeta::DataBar(meta) = symbol.meta else {
        panic!("expected DataBar meta");
    };
    assert_eq!(meta.variant, DataBarVariant::Stacked);
    assert_eq!(meta.columns_per_row, None);
}
