//! Code 39 end-to-end tests: independent reference vectors plus encode → decode →
//! re-encode identity across the option matrix (standard/full-ASCII, with/without a
//! mod-43 check character).
#![cfg(all(feature = "decode", feature = "encode", feature = "code39"))]

use anyd::codes::code39::{Code39Decoder, Code39Encoder};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

/// Turn a `1`/`0` string into a module vector (`true` = bar).
fn bits(s: &str) -> Vec<bool> {
    s.bytes().map(|b| b == b'1').collect()
}

fn modules(enc: &Encoding) -> &Vec<bool> {
    match enc {
        Encoding::Linear(p) => &p.modules,
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

/// Reference vector: `*A*` assembled from the published Code 39 patterns
/// (start/stop `*` = 100101101101, `A` = 110101001011) joined by narrow gaps.
/// Source: Bar Code Island Code 39 specification.
#[test]
fn reference_vector_star_a_star() {
    let enc = Code39Encoder::new();
    let symbol = enc.build(b"A", false, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let star = "100101101101";
    let a = "110101001011";
    let expected = bits(&format!("{star}0{a}0{star}"));
    assert_eq!(modules(&encoding), &expected);
}

/// Reference vector: the mod-43 check character of "ABC" is 33 = `X`
/// (10 + 11 + 12 = 33). Encoding "ABC" with a check must equal encoding the literal
/// "ABCX" without one.
#[test]
fn reference_vector_mod43_check_abc_is_x() {
    let enc = Code39Encoder::new();
    let with_check = enc
        .encode(&enc.build(b"ABC", false, true).unwrap())
        .unwrap();
    let literal = enc
        .encode(&enc.build(b"ABCX", false, false).unwrap())
        .unwrap();
    assert_eq!(modules(&with_check), modules(&literal));
}

/// Full assert: decode recovers the exact segments and meta, and the decoded symbol
/// re-encodes byte-for-byte.
fn assert_roundtrip(data: &[u8], full_ascii: bool, check_digit: bool) {
    let enc = Code39Encoder::new();
    let dec = Code39Decoder::new()
        .with_full_ascii(full_ascii)
        .with_check_digit(check_digit);

    let symbol = enc.build(data, full_ascii, check_digit).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = dec.decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    match &decoded.meta {
        SymbolMeta::Code39(m) => {
            assert_eq!(m.full_ascii, full_ascii);
            assert_eq!(m.check_digit, check_digit);
        }
        _ => panic!("wrong meta variant"),
    }

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_standard_option_matrix() {
    for &check in &[false, true] {
        assert_roundtrip(b"CODE39", false, check);
        assert_roundtrip(b"ABC-123 $/+%.", false, check);
        assert_roundtrip(b"0123456789", false, check);
        assert_roundtrip(b"", false, check);
    }
}

#[test]
fn roundtrip_full_ascii_option_matrix() {
    for &check in &[false, true] {
        assert_roundtrip(b"Hello, World!", true, check);
        assert_roundtrip(b"abc-XYZ_123", true, check);
        // Base characters via full-ASCII still round-trip (as a byte segment).
        assert_roundtrip(b"ABC 123", true, check);
    }
}

#[test]
fn roundtrip_all_ascii_bytes() {
    // Every ASCII byte through the full-ASCII shift machinery, with a check character.
    let all: Vec<u8> = (0u8..128).collect();
    assert_roundtrip(&all, true, true);
}

#[test]
fn decoded_payload_is_recoverable_text() {
    let enc = Code39Encoder::new();
    let symbol = enc.build(b"Hello!", true, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = Code39Decoder::new()
        .with_full_ascii(true)
        .decode(&encoding)
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("Hello!"));
    assert_eq!(decoded.segments, vec![Segment::byte(b"Hello!".to_vec())]);
}

#[test]
fn corrupt_check_is_rejected() {
    let enc = Code39Encoder::new();
    // Build without a check, then decode demanding one: the last data char is treated
    // as a check and will almost never validate.
    let symbol = enc.build(b"CODE39", false, false).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let result = Code39Decoder::new()
        .with_check_digit(true)
        .decode(&encoding);
    assert!(result.is_err());
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::codes::code39::Code39Meta;
    use anyd::output::LinearBuf;
    let enc = Code39Encoder::new();
    for (data, full_ascii) in [
        (&b"CODE39"[..], false),
        (b"", false),
        (b"HELLO-WORLD 123 $/+%", false),
        (b"Hello, World!\x00\x7f", true),
    ] {
        for check_digit in [false, true] {
            let symbol = enc.build(data, full_ascii, check_digit).unwrap();
            let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
                panic!("Code 39 encodes to a linear pattern");
            };
            let meta = Code39Meta {
                full_ascii,
                check_digit,
            };
            let mut storage = [0u8; 128];
            let mut buf = LinearBuf::new(&mut storage);
            enc.encode_into(data, &meta, &mut buf).unwrap();
            assert_eq!(buf, expected);
            assert!(buf.len() <= Code39Encoder::max_modules(data.len(), &meta));
            if !full_ascii {
                assert_eq!(buf.len(), Code39Encoder::max_modules(data.len(), &meta));
            }
            // Too-small storage reports a capacity error instead of panicking.
            let mut tiny = [0u8; 2];
            let err = enc.encode_into(data, &meta, &mut LinearBuf::new(&mut tiny));
            assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        }
    }
}

/// Re-draw a 2:1 module row with every two-module element widened to `wide` modules.
fn with_wide_ratio(modules: &[bool], wide: usize) -> Vec<bool> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < modules.len() {
        let mut j = i;
        while j < modules.len() && modules[j] == modules[i] {
            j += 1;
        }
        let width = if j - i == 2 { wide } else { j - i };
        out.extend(std::iter::repeat_n(modules[i], width));
        i = j;
    }
    out
}

/// ISO/IEC 16388 allows a wide:narrow ratio anywhere from 2:1 to 3:1, and most
/// generators print 3:1. The decoder must read those, not only this crate's 2:1.
#[test]
fn decodes_three_to_one_ratio_symbols() {
    let enc = Code39Encoder::new();
    for (data, full_ascii, check) in [
        (&b"CODE-39"[..], false, false),
        (b"A", false, true),
        (b"Hello, World!", true, true),
    ] {
        let symbol = enc.build(data, full_ascii, check).unwrap();
        let narrow = enc.encode(&symbol).unwrap();
        let mut pattern = anyd::output::LinearPattern::new();
        pattern.modules = with_wide_ratio(modules(&narrow), 3);
        let decoded = Code39Decoder::new()
            .with_full_ascii(full_ascii)
            .with_check_digit(check)
            .decode(&Encoding::Linear(pattern))
            .unwrap();
        assert_eq!(decoded.segments, symbol.segments);
        // The ratio is not part of the symbol: it re-encodes to the canonical 2:1 row.
        assert_eq!(enc.encode(&decoded).unwrap(), narrow);
    }
}
