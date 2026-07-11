//! MaxiCode (ISO/IEC 16023) tests.
//!
//! Two kinds of coverage:
//!
//! 1. **Independent reference vectors.** Two module bitmaps produced by *zint*
//!    (cross-checked there against BWIPP and ZXing-C++, and derived from the
//!    ISO/IEC 16023:2000 figures) are embedded verbatim:
//!    - Figure 2 / L1 — a Mode 4 symbol of a 93-character Code Set A message. Our
//!      encoder must reproduce this bitmap *exactly*, which pins the GF(64) Reed–
//!      Solomon, the interleave and the module-placement grid end to end.
//!    - Figure B2 — the canonical Mode 2 UPS symbol (postcode 152382802, country
//!      840, service 001). Our decoder must recover the Structured Carrier fields
//!      and the secondary message, and the block must satisfy every RS parity check.
//!
//!    The placement grid itself is additionally cross-validated in the source: zint's
//!    `maxiGrid[n]` equals ZXing's `BITNR[n] + 1`.
//!
//! 2. **Lossless round-trips.** `encode(decode(encode(build(x)))) == encode(build(x))`
//!    across modes 4, 5, 6 and 2/3, plus recovery from correctable errors.

use anyd::codes::maxicode::{MaxiCodeDecoder, MaxiCodeEncoder, MaxiCodeMeta};
use anyd::output::{BitMatrix, Encoding};
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

/// Build a 30×33 [`BitMatrix`] from 33 rows of `'0'`/`'1'` (top to bottom).
fn matrix_from_rows(rows: &[&str]) -> BitMatrix {
    assert_eq!(rows.len(), 33);
    let mut m = BitMatrix::new(30, 33, 1);
    for (y, row) in rows.iter().enumerate() {
        assert_eq!(row.len(), 30, "row {y} wrong width");
        for (x, ch) in row.bytes().enumerate() {
            if ch == b'1' {
                m.set(x, y, true);
            }
        }
    }
    m
}

fn meta(sym: &anyd::Symbol) -> &MaxiCodeMeta {
    match &sym.meta {
        SymbolMeta::MaxiCode(m) => m,
        _ => panic!("expected MaxiCode meta"),
    }
}

/// zint Mode 4 rendering of the ISO/IEC 16023:2000 Figure 2 message.
const ISO_FIG2_MODE4: [&str; 33] = [
    "011111010000001000001000100111",
    "000100000001000000001010000000",
    "001011001100100110110010010010",
    "100000010001100010010000000000",
    "001011000000101000001010110011",
    "111010001000001011001000111100",
    "100000000110000010010000000000",
    "000010100010010010001001111100",
    "111011100000001000000110000000",
    "000000011011000000010100011000",
    "101111000001010110001100000011",
    "001110001010000000111010001110",
    "000111100000000000100001011000",
    "100010000000000000000111001000",
    "100000001000000000011000001000",
    "000010111000000000000010000010",
    "111000001000000000001000001101",
    "011000000000000000001000100100",
    "000000101100000000001001010001",
    "101010001000000000100111001100",
    "001000011000000000011100001010",
    "000000000000000000110000100000",
    "101011001010100001000101010001",
    "100011110010101001101010001010",
    "011010000000000101011010011111",
    "000001110011111111111100010100",
    "001110100111000101011000011100",
    "110111011100100001101001010110",
    "000001011011101010010111001100",
    "111000110111100010001111011110",
    "101111010111111000010110111001",
    "001001101111101101101010011100",
    "001011000000111101100100001000",
];

/// zint Mode 2 rendering of the ISO/IEC 16023:2000 Figure B2 UPS symbol.
const ISO_FIGB2_MODE2: [&str; 33] = [
    "110101110110111110111111101111",
    "010101010111000011011000010010",
    "110110110001001010100110010001",
    "111000101010101111111111111110",
    "001111000010110010011000000011",
    "001001110010101010100000000000",
    "111011111110111111101111111110",
    "100110000011001001110000001010",
    "010001100010101010101001110001",
    "110111100011010000011011111100",
    "001100110011110000001110101001",
    "101110101000000001011111011010",
    "101010000000000000010110111111",
    "111101100000000000011011100010",
    "101010010000000000000110011101",
    "001000010000000000011100011110",
    "010011001000000000001000001010",
    "000000101000000000001010000010",
    "000100111100000000001110101010",
    "000010101100000000001000110000",
    "100000111010000000011101100000",
    "101000100000000000110110100000",
    "001000001110100101100110100101",
    "011001110010101001100000001000",
    "000010100010110001010101111010",
    "100111000011111000001101101010",
    "110010001001010010100000001101",
    "000001000110110100111010111100",
    "010111010010100100001111011010",
    "110111001110101010101101110100",
    "011011110001010100010111010011",
    "000111101001100001111000010110",
    "000100101000110000000111110011",
];

/// The 93-character Code Set A message behind [`ISO_FIG2_MODE4`].
const FIG2_MESSAGE: &str =
    "THIS IS A 93 CHARACTER CODE SET A MESSAGE THAT FILLS A MODE 4, UNAPPENDED, MAXICODE SYMBOL...";

#[test]
fn encoder_matches_iso_figure2_bitmap() {
    let enc = MaxiCodeEncoder::new();
    let sym = enc.build(FIG2_MESSAGE.as_bytes()).unwrap();
    let Encoding::Matrix(got) = enc.encode(&sym).unwrap() else {
        panic!("expected matrix");
    };
    let want = matrix_from_rows(&ISO_FIG2_MODE4);
    for y in 0..33 {
        for x in 0..30 {
            assert_eq!(
                got.get(x, y),
                want.get(x, y),
                "module ({x},{y}) differs from zint's ISO Figure 2 bitmap"
            );
        }
    }
}

#[test]
fn decodes_iso_figureb2_ups_symbol() {
    let m = matrix_from_rows(&ISO_FIGB2_MODE2);
    let sym = MaxiCodeDecoder::new().decode(&Encoding::Matrix(m)).unwrap();
    let meta = meta(&sym);
    assert_eq!(meta.mode, 2);
    let carrier = meta.carrier.as_ref().expect("mode 2 carrier");
    assert_eq!(carrier.postcode, "152382802");
    assert_eq!(carrier.country, 840);
    assert_eq!(carrier.service, 1);

    let text = sym.text().expect("utf-8 payload");
    // The secondary message carries the UPS tracking data (after the SCM prefix).
    assert!(
        text.contains("1Z00004951"),
        "missing tracking number: {text:?}"
    );
    assert!(text.contains("UPSN"), "missing UPSN: {text:?}");
    assert!(text.contains("PITTSBURGH"), "missing city: {text:?}");
}

/// Full lossless invariant: decode recovers identical segments + meta, and the
/// decoded symbol re-encodes byte-for-byte.
fn assert_lossless(sym: &anyd::Symbol) {
    let enc = MaxiCodeEncoder::new();
    let encoding = enc.encode(sym).unwrap();
    let decoded = MaxiCodeDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, sym.segments, "segments differ");
    assert_eq!(decoded.meta, sym.meta, "meta differs");
    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn roundtrip_mode4_text() {
    let enc = MaxiCodeEncoder::new();
    for s in [
        "HELLO WORLD",
        "MaxiCode 16023",
        "ABCDabcd1234!?",
        "The quick brown fox jumps over 13 lazy dogs.",
    ] {
        let sym = enc.build_text(s).unwrap();
        assert_eq!(sym.text().as_deref(), Some(s));
        assert_lossless(&sym);
    }
}

#[test]
fn roundtrip_mode4_high_bytes() {
    let enc = MaxiCodeEncoder::new();
    let data: Vec<u8> = [0x00u8, 0x80, 0xC0, 0xE0, 0xFF, 0x1B, 0x41, 0x61, 0xA0].to_vec();
    let sym = enc.build_mode(&data, 4).unwrap();
    assert_eq!(sym.payload_bytes(), data);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_mode5_enhanced_ec() {
    let enc = MaxiCodeEncoder::new();
    let sym = enc.build_mode(b"ENHANCED EC MODE 5", 5).unwrap();
    let m = meta(&sym);
    assert_eq!(m.mode, 5);
    assert_eq!(m.body.len(), 77);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_mode6_reader_programming() {
    let enc = MaxiCodeEncoder::new();
    let sym = enc.build_mode(b"READER CONFIG 42", 6).unwrap();
    assert_eq!(meta(&sym).mode, 6);
    assert_lossless(&sym);
}

#[test]
fn roundtrip_structured_mode2() {
    let enc = MaxiCodeEncoder::new();
    let sym = enc
        .build_structured(2, "152382802", 840, 1, b"1Z00004951")
        .unwrap();
    let m = meta(&sym);
    assert_eq!(m.mode, 2);
    let c = m.carrier.as_ref().unwrap();
    assert_eq!(
        (c.postcode.as_str(), c.country, c.service),
        ("152382802", 840, 1)
    );
    assert_lossless(&sym);
}

#[test]
fn roundtrip_structured_mode3() {
    let enc = MaxiCodeEncoder::new();
    let sym = enc
        .build_structured(3, "B1050", 56, 999, b"CEN DATA")
        .unwrap();
    let m = meta(&sym);
    assert_eq!(m.mode, 3);
    let c = m.carrier.as_ref().unwrap();
    // Mode 3 postcodes are upper-cased and space-padded to 6 characters.
    assert_eq!(c.postcode, "B1050 ");
    assert_eq!((c.country, c.service), (56, 999));
    assert_lossless(&sym);
}

#[test]
fn survives_correctable_errors() {
    let enc = MaxiCodeEncoder::new();
    let sym = enc.build_mode(b"ERROR CORRECTION TEST 2026", 4).unwrap();
    let Encoding::Matrix(mut m) = enc.encode(&sym).unwrap() else {
        panic!("expected matrix");
    };
    // Flip a spread of interior data modules; Standard EC corrects up to 10 symbol
    // errors per secondary interleave.
    for k in 0..8 {
        let x = 2 + k * 3;
        let y = 24 + (k % 6);
        let v = m.get(x, y);
        m.set(x, y, !v);
    }
    let decoded = MaxiCodeDecoder::new().decode(&Encoding::Matrix(m)).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("ERROR CORRECTION TEST 2026")
    );
    assert_eq!(decoded.meta, sym.meta);
}

#[test]
fn rejects_wrong_size_grid() {
    let m = BitMatrix::new(30, 30, 1);
    assert!(MaxiCodeDecoder::new().decode(&Encoding::Matrix(m)).is_err());
}
