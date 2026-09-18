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
#![cfg(all(feature = "decode", feature = "encode", feature = "databar"))]

use anyd::codes::databar::{DataBarDecoder, DataBarEncoder, DataBarMeta, DataBarVariant};
use anyd::output::Encoding;
use anyd::segment::Mode;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

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

// ---- Encodation methods 3-14 (zint 2.16.0 `--dump` reference vectors) ----

/// The variable-weight / price / date encodation methods of ISO/IEC 24724:2011
/// §7.2.5.4, and the boundaries where an element string falls back to a more general
/// method. Every pattern is zint 2.16.0's output for the bracketed input
/// (`zint -b DBAR_EXP --gs1nocheck -d ... --dump`); each must be reproduced exactly
/// and must decode back to the same reduced element string.
#[test]
fn compressed_method_reference_vectors() {
    let enc = DataBarEncoder::new();
    let dec = DataBarDecoder::new();
    let cases: &[(&[u8], &str)] = &[
        // method 3 `0100`: (3103) weight <= 32767: `[01]90012345678908[3103]001750`.
        (
            b"01900123456789083103001750",
            "0101110010000010011011111111000010111000010011000101011110111001100010111100000011100101110001110111011110101111000110001111110000101011000010011111010",
        ),
        // method 3 upper bound: `[01]90012345678908[3103]032767`.
        (
            b"01900123456789083103032767",
            "0101001000111000011011111111000010111000010011000101011110111001100010111100000011100101110001110111011110111011000110001111110000101101111101110111010",
        ),
        // one past method 3: method 7 without a date: `[01]90012345678908[3103]032768`.
        (
            b"01900123456789083103032768",
            "01001100000101001110111111110000101000110111000010010111100110111110101111110000111000101100001101110001111011010111100011111100001011000110100110000110100000100110001011111111001110011001111010000101",
        ),
        // method 4 `0101`: (3202) weight <= 9999: `[01]90012345678908[3202]000156`.
        (
            b"01900123456789083202000156",
            "0101001000111100001011111111000010100111000100001101011110111001100010111100000011100101110001110111011110101111000110001111110000101100001000001010010",
        ),
        // method 4: (3203) weight <= 22767, stored + 10000: `[01]90012345678908[3203]022767`.
        (
            b"01900123456789083203022767",
            "0101110010011000001011111111000010100111000100001101011110111001100010111100000011100101110001110111011110111011000110001111110000101101111101110111010",
        ),
        // one past method 4: method 8 without a date: `[01]90012345678908[3203]022768`.
        (
            b"01900123456789083203022768",
            "01000110111000100010111111110000101110100011000010010111100110111110101111110000111000101100001101110001111011010111100011111100001010011100010110000100001101000001101011111111001110011001111010000101",
        ),
        // method 5 `01100`: (392x) price: `[01]90012345678908[3922]795`.
        (
            b"01900123456789083922795",
            "010110000010001011101111111100001010011100000101100101111001101111101011111100001110001011000011011100011110110101111000111111000010100111101110100001100011011100100010111111110011101",
        ),
        // method 6 `01101`: (393x) currency + price: `[01]90012345678908[3932]0401234`.
        (
            b"019001234567890839320401234",
            "01000111101010000010111111110000101110100000110010010111100110111110101111110000111000101100001101110001111011010111100011111100001011100011001101100100111110001011101011111111001110001101111001011101",
        ),
        // method 7: (310x) + (11) production date: `[01]90012345678908[3102]001750[11]100312`.
        (
            b"0190012345678908310200175011100312",
            "01001000011101000110111111110000101000110111000010010111100110111110101111110000111000101100001101110001111011010111100011111100001011110010000010100101001110111110001011111111001110000001010010011101",
        ),
        // method 8: (320x) + (11): `[01]90012345678908[3205]099999[11]201231`.
        (
            b"0190012345678908320509999911201231",
            "01001000001001110110111111110000101110100011000010010111100110111110101111110000111000101100001101110001111011010111100011111100001011110011010001100101001100011000001011111111001110010010001000011101",
        ),
        // method 9: (310x) + (13) packaging date: `[01]90012345678908[3100]000100[13]991200`.
        (
            b"0190012345678908310000010013991200",
            "01001100000111010010111111110000101111001010000010010111100110111110101111110000111000101100001101110001111011010111100011111100001011000000010001010101100001101110001011111111001110110000111100010101",
        ),
        // method 10: (320x) + (13): `[01]90012345678908[3201]000100[13]000101`.
        (
            b"0190012345678908320100010013000101",
            "01000101000110001110111111110000101100101000001110010111100110111110101111110000111000101100001101110001111011010111100011111100001010001111000001010101110111111011001011111111001110111111100111010101",
        ),
        // method 11: (310x) + (15) best-before date: `[01]90012345678908[3103]012233[15]991231`.
        (
            b"0190012345678908310301223315991231",
            "01001100000100111010111111110000101011100100000110010111100110111110101111110000111000101100001101110001111011010111100011111100001011000011010110000111001100110001001011111111001110001101111010000101",
        ),
        // method 12: (320x) + (15): `[01]98898765432106[3202]012345[15]991231`.
        (
            b"0198898765432106320201234515991231",
            "01001000011000110110111111110000101110000110010100011010000001100010101111110000111010011100000010010100111110111001100011111100001011101100000100100100011110010110001011111111001110001101111010000101",
        ),
        // method 13: (310x) + (17) expiry date: `[01]90012345678908[3109]001750[17]010101`.
        (
            b"0190012345678908310900175017010101",
            "01001100000101001110111111110000101110000100110100010111100110111110101111110000111000101100001101110001111011010111100011111100001011110111010001110100111000001110101011111111001110111100111101000101",
        ),
        // method 14: (320x) + (17): `[01]90012345678908[3209]001750[17]010101`.
        (
            b"0190012345678908320900175017010101",
            "01000101101100000110111111110000101111000100010100010111100110111110101111110000111000101100001101110001111011010111100011111100001011110111010001110100111000001110101011111111001110111100111101000101",
        ),
        // not method 7: month 13 falls back to method 1: `[01]90012345678908[3103]001750[11]991300`.
        (
            b"0190012345678908310300175011991300",
            "01011001101100001110111111110000101001000111110011010111100110111110101111100000011000101100001101110001111011010111100011111100001010010110001110000100001000011011001011111111001110110010110000011100001100010100111000110000000010110000011000110101010111101111110010111111111001101",
        ),
        // not method 3: indicator digit 8 falls back to method 1: `[01]80012345678901[3103]001750`.
        (
            b"01800123456789013103001750",
            "01000110110010000110111111110000101110010110000100010111100110111110101111110000111000101100001101110001111011010111100011111100001010010110001110000100001000011011001011111111001110110010110000011101",
        ),
        // not method 7: six significant weight digits: `[01]90012345678908[3103]123456`.
        (
            b"01900123456789083103123456",
            "01001100001011000110111111110000101111010010000100010111100110111110101111110000111000101100001101110001111011010111100011111100001010010110001110000100001000011100101011111111001110100110011000000101",
        ),
        // method 5 with a following AI in the general field: `[01]90012345678908[3922]1[10]AB`.
        (
            b"019001234567890839221\x1d10AB",
            "01000101101100000110111111110000101111000100001010010111100110111110101111110000111000101100001101110001111011010111100011111100001011110100011100100110100011110011101011111111001110011100011101110101",
        ),
    ];
    for (reduced, expected) in cases {
        let symbol = enc.build_expanded(reduced).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        assert_eq!(
            &bits(&encoding),
            expected,
            "expanded module pattern mismatch for {:?}",
            String::from_utf8_lossy(reduced)
        );
        let decoded = dec.decode(&encoding).unwrap();
        assert_eq!(decoded, symbol);
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
fn roundtrip_compressed_method_sweep() {
    // Indicator-9 GTINs with every (310x)/(320x)/(392x)/(393x) AI, sweeping weights
    // across the method 3/4 limits, with and without each date AI, plus prices.
    let weights = [
        0u32, 1, 9_999, 10_000, 22_767, 22_768, 32_767, 32_768, 99_999, 100_000, 999_999,
    ];
    let dates: [&[u8]; 6] = [b"", b"000100", b"991231", b"240229", b"001300", b"991232"];
    for ai3 in [b"310", b"320"] {
        for x in b'0'..=b'9' {
            for weight in weights {
                for date_ai in [&b"11"[..], b"13", b"15", b"17", b"12"] {
                    for date in dates {
                        let mut reduced = b"0190012345678908".to_vec();
                        reduced.extend_from_slice(ai3);
                        reduced.push(x);
                        reduced.extend_from_slice(format!("{weight:06}").as_bytes());
                        if !date.is_empty() {
                            reduced.extend_from_slice(date_ai);
                            reduced.extend_from_slice(date);
                        }
                        assert_roundtrip(&reduced);
                    }
                }
            }
        }
    }
    for ai in [&b"3920"[..], b"3923", b"3924", b"3930", b"3933", b"3934"] {
        for value in [
            &b"0"[..],
            b"978",
            b"9785",
            b"840123456789012",
            b"97812\x1d10AB",
        ] {
            let mut reduced = b"0190012345678908".to_vec();
            reduced.extend_from_slice(ai);
            reduced.extend_from_slice(value);
            assert_roundtrip(&reduced);
        }
    }
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
