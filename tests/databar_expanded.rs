//! GS1 DataBar Expanded (non-stacked) end-to-end tests.
//!
//! Two kinds of check:
//! 1. **Independent reference vectors** — exact module patterns for known GS1
//!    element strings, taken from zint's `test_rss.c` `test_examples` data, which
//!    are documented there as "verified via bwipp_dump.ps against BWIPP" and
//!    manually against GS1 General Specifications / ISO/IEC 24724:2011 and tec-it.
//!    That toolchain (BWIPP / tec-it) is independent of this crate's algorithm.
//!    `1` = dark module, `0` = light, left to right.
//! 2. **Lossless round-trip** — encode -> decode -> re-encode reproduces the exact
//!    segments, metadata and module pattern across numeric, alphanumeric, ISO/IEC
//!    646 and FNC1-separated payloads.
//!
//! Payloads are supplied in *reduced* form: AI numbers + values concatenated, with
//! the GS1 separator FNC1 encoded as byte `0x1D`. This matches zint's internal
//! "reduced" element string, so the reference module patterns line up.

use anydcode::codes::databar::{DataBarDecoder, DataBarEncoder, DataBarMeta, DataBarVariant};
use anydcode::output::Encoding;
use anydcode::segment::Mode;
use anydcode::symbol::SymbolMeta;
use anydcode::symbology::Symbology;
use anydcode::traits::{Decode, Encode};

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

// ---- Independent reference vectors (zint test_examples / BWIPP / tec-it) ----

#[test]
fn expanded_reference_vectors() {
    let enc = DataBarEncoder::new();
    // (reduced element string, expected module pattern).
    // GS1 separator FNC1 is byte 0x1D (`\x1d`).
    let cases: &[(&[u8], &str)] = &[
        // zint test_examples #15: `[10]12A` (encoding method 2, alphanumeric).
        (
            b"1012A",
            "010100000110100000101111111100001010001000000010110101111100100111001011110000000010011101111111010101",
        ),
        // zint test_examples #17: `[255]95011015340010123456789` (method 2, numeric).
        (
            b"25595011015340010123456789",
            "0100011000110001011011111111000010100000010101100001100001100111001010111110000001100100001110100001001000011011111010001111110000101001011111100111011001000111100100101111111100111011111001100100110010011100010111100011110000001010",
        ),
        // zint test_examples #25: `[01]09501101530003[17]140704[10]AB-123`
        // (encoding method 1: (01) GTIN compressed field + alphanumeric general field).
        (
            b"01095011015300031714070410AB-123",
            "01010111110000100110111111110000101110000111011001010000001101101000101111100000011001101000000100110001110100010001100011111100001010100000111100100100100111000001001011111111001110000011011001000100010000101000011000110000000010110000110011010001101100111000001010111111111001101",
        ),
        // zint test_examples #19: `[255]9501101534001[17]160531[3902]050`
        // (method 2 with an embedded FNC1 separator after the variable-length (255)).
        (
            b"2559501101534001\x1d171605313902050",
            "01011001000110011110111111110000101000000101011000011000011001110010101111100000011001000011101000010010000110111110100011111100001010010111111001110111000010010100001011111111001110000100001100110100010000001101001000110000000010111010011110011101110010110001100010111111111001101",
        ),
    ];
    for (reduced, expected) in cases {
        let symbol = enc.build_expanded(reduced).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        assert_eq!(
            &bits(&encoding),
            expected,
            "expanded module pattern mismatch for {reduced:?}"
        );
    }
}

// ---- Canonical form: build stores the reduced element string as one byte segment ----

#[test]
fn build_stores_reduced_byte_segment() {
    let enc = DataBarEncoder::new();
    let symbol = enc.build_expanded(b"1012A").unwrap();
    assert_eq!(symbol.symbology, Symbology::DataBarExpanded);
    assert_eq!(symbol.segments.len(), 1);
    assert_eq!(symbol.segments[0].mode, Mode::Byte);
    assert_eq!(symbol.segments[0].data, b"1012A");
    assert_eq!(
        symbol.meta,
        SymbolMeta::DataBar(DataBarMeta::new(DataBarVariant::Expanded))
    );
}

// ---- Lossless round-trips ----

fn assert_roundtrip(reduced: &[u8]) {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let symbol = enc.build_expanded(reduced).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(
        decoded.symbology,
        Symbology::DataBarExpanded,
        "symbology ({reduced:?})"
    );
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ ({reduced:?})"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs ({reduced:?})");
    assert_eq!(
        enc.encode(&decoded).unwrap(),
        encoding,
        "re-encode not identical ({reduced:?})"
    );
}

#[test]
fn roundtrip_numeric() {
    assert_roundtrip(b"0100000000000017"); // (01) GTIN, method 1, no general field
    assert_roundtrip(b"25595011015340010123456789");
    assert_roundtrip(b"100000000000"); // long numeric (method 2)
    assert_roundtrip(b"1012"); // short, ends on even numeric
    assert_roundtrip(b"10123"); // short, ends on lone (odd) digit
}

#[test]
fn roundtrip_alphanumeric() {
    assert_roundtrip(b"1012A");
    assert_roundtrip(b"10ABCDEFG");
    assert_roundtrip(b"10AB-123");
    assert_roundtrip(b"10A1B2C3D4"); // alternating alpha/numeric
    assert_roundtrip(b"10ABC123XYZ789");
}

#[test]
fn roundtrip_isoiec646() {
    assert_roundtrip(b"10abcdef"); // lowercase forces ISO/IEC 646 mode
    assert_roundtrip(b"10a1b2c3");
    assert_roundtrip(b"10Aa!Bb?"); // mixed case + ISO/IEC punctuation
    assert_roundtrip(b"10x=y_z:w");
}

#[test]
fn roundtrip_mixed_and_fnc1() {
    assert_roundtrip(b"01095011015300031714070410AB-123");
    assert_roundtrip(b"2559501101534001\x1d171605313902050");
    assert_roundtrip(b"10ABC\x1d213456"); // alpha, FNC1 separator, numeric
    assert_roundtrip(b"10abc\x1d213456"); // ISO/IEC, FNC1, numeric
    assert_roundtrip(b"3712\x1d10ABCabc123"); // numeric, FNC1, mixed
}

#[test]
fn roundtrip_method1_variants() {
    // (01) GTIN with varying following AIs (all method 1 as GTIN starts with a
    // non-9 indicator so no weight/date shortcut applies).
    assert_roundtrip(b"0100000000000017"); // GTIN only
    assert_roundtrip(b"010950110153000317140704"); // GTIN + fixed-length (17)
    assert_roundtrip(b"01095011015300031012345"); // GTIN + variable (10) numeric
    assert_roundtrip(b"0109501101530003102ABC"); // GTIN + variable (10) alphanumeric
}

#[test]
fn roundtrip_sweep() {
    // A deterministic pseudo-random sweep over many lengths and character mixes
    // exercises every mode transition, the odd-digit terminators, and the padding
    // termination across the full range of symbol-character counts. A single
    // mis-inverted latch or pad bit would corrupt the decoded payload.
    let alphabet: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz*-. :?_";
    let mut state: u64 = 0x1234_5678_9abc_def1;
    let mut next = || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    let enc = DataBarEncoder::new();
    let mut exercised = 0;
    for _ in 0..3000 {
        let len = 1 + next() % 40;
        let payload: Vec<u8> = (0..len)
            .map(|_| alphabet[next() % alphabet.len()])
            .collect();
        // Long ISO/IEC-heavy payloads can exceed the 21-symbol-character limit; that
        // is a legitimate capacity error, so only round-trip payloads that fit.
        if enc.build_expanded(&payload).is_ok() {
            assert_roundtrip(&payload);
            exercised += 1;
        }
    }
    assert!(
        exercised > 2000,
        "sweep exercised too few payloads: {exercised}"
    );
}

// ---- Length growth: symbol grows with more finder patterns / symbol characters ----

#[test]
fn symbol_grows_with_data() {
    let enc = DataBarEncoder::new();
    // Numeric payloads of increasing length cross several symbol-character
    // boundaries, adding finder patterns and widening the symbol.
    let payloads: [Vec<u8>; 4] = [
        b"10".to_vec(),
        [b"10".as_slice(), &[b'0'; 20]].concat(),
        [b"10".as_slice(), &[b'0'; 40]].concat(),
        [b"10".as_slice(), &[b'0'; 60]].concat(),
    ];
    let mut prev = 0;
    let mut widths = Vec::new();
    for n in &payloads {
        let Encoding::Linear(p) = enc.encode(&enc.build_expanded(n).unwrap()).unwrap() else {
            panic!("linear");
        };
        assert!(
            p.modules.len() >= prev,
            "expected non-decreasing width: {} vs {}",
            p.modules.len(),
            prev
        );
        prev = p.modules.len();
        widths.push(p.modules.len());
    }
    // The longest payload must yield a strictly wider symbol than the shortest.
    assert!(widths.last().unwrap() > widths.first().unwrap());
}

// ---- Error handling ----

#[test]
fn rejects_bad_gtin_check_digit() {
    let enc = DataBarEncoder::new();
    // Method 1 input whose (01) check digit is wrong (correct final digit is 7).
    assert!(enc.build_expanded(b"0100000000000010").is_err());
}

#[test]
fn rejects_invalid_character() {
    let enc = DataBarEncoder::new();
    // A backtick is outside the general-field character set.
    assert!(enc.build_expanded(b"10ABC`123").is_err());
}

#[test]
fn rejects_empty_payload() {
    let enc = DataBarEncoder::new();
    assert!(enc.build_expanded(b"").is_err());
}

#[test]
fn decode_rejects_corrupted_expanded() {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let symbol = enc.build_expanded(b"25595011015340010123456789").unwrap();
    let Encoding::Linear(mut p) = enc.encode(&symbol).unwrap() else {
        panic!("expected linear");
    };
    // Flip several interior data modules to break the checksum.
    for i in [30, 31, 60, 61, 90] {
        p.modules[i] = !p.modules[i];
    }
    assert!(dec.decode(&Encoding::Linear(p)).is_err());
}
