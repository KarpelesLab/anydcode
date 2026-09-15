//! Code 93 end-to-end tests: an independent checksum reference vector plus
//! encode → decode → re-encode identity across standard and full-ASCII payloads.
#![cfg(all(feature = "decode", feature = "encode", feature = "code93"))]

use anyd::codes::code93::{Code93Decoder, Code93Encoder};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

fn modules(enc: &Encoding) -> Vec<bool> {
    match enc {
        Encoding::Linear(p) => p.modules.clone(),
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

/// Whether a byte is one of the 43 base Code 93 characters.
fn is_base(b: u8) -> bool {
    b.is_ascii_digit()
        || b.is_ascii_uppercase()
        || matches!(b, b'-' | b'.' | b' ' | b'$' | b'/' | b'+' | b'%')
}

/// Reference vector: for `TEST93` the two check characters are C = `+` and K = `6`
/// (Bar Code Island Code 93 specification). Encoding `TEST93` must therefore equal
/// encoding the manually-appended `TEST93+6` reconstructed from the pattern table.
///
/// We assert it indirectly and robustly: the full symbol is
/// `* T E S T 9 3 + 6 *` + terminating bar. Build the expected module string from the
/// published nine-module binary patterns.
#[test]
fn reference_vector_test93_checks() {
    // Published nine-module patterns (Wikipedia Code 93 table).
    let pat = |c: char| -> &'static str {
        match c {
            '*' => "101011110",
            'T' => "110100110",
            'E' => "110010010",
            'S' => "110101100",
            '9' => "100001010",
            '3' => "101000010",
            '+' => "101110110", // value 41 -> C
            '6' => "100100010", // value 6  -> K
            _ => unreachable!(),
        }
    };
    let mut expected = String::new();
    for c in "*TEST93+6*".chars() {
        expected.push_str(pat(c));
    }
    expected.push('1'); // terminating bar
    let expected: Vec<bool> = expected.bytes().map(|b| b == b'1').collect();

    let enc = Code93Encoder::new();
    let symbol = enc.build(b"TEST93", false).unwrap();
    assert_eq!(modules(&enc.encode(&symbol).unwrap()), expected);
}

fn assert_roundtrip(data: &[u8]) {
    let full_ascii = data.iter().any(|&b| !is_base(b));
    let enc = Code93Encoder::new();
    let symbol = enc.build(data, full_ascii).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = Code93Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments, "segments differ");
    assert_eq!(decoded.meta, symbol.meta, "meta differs");
    match &decoded.meta {
        SymbolMeta::Code93(m) => assert_eq!(m.full_ascii, full_ascii),
        _ => panic!("wrong meta variant"),
    }

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encode not byte-identical");
}

#[test]
fn roundtrip_standard() {
    assert_roundtrip(b"TEST93");
    assert_roundtrip(b"CODE 93");
    assert_roundtrip(b"WIKIPEDIA");
    assert_roundtrip(b"ABC-123 $/+%.");
    assert_roundtrip(b"0123456789");
    assert_roundtrip(b"");
}

#[test]
fn roundtrip_full_ascii() {
    assert_roundtrip(b"Hello, World!");
    assert_roundtrip(b"lower+CASE=mix");
    assert_roundtrip(b"a\tb\nc"); // control characters
}

#[test]
fn roundtrip_all_ascii_bytes() {
    let all: Vec<u8> = (0u8..128).collect();
    assert_roundtrip(&all);
}

#[test]
fn decoded_text_recovers() {
    let enc = Code93Encoder::new();
    let symbol = enc.build(b"Hello!", true).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = Code93Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("Hello!"));
    assert_eq!(decoded.segments, vec![Segment::byte(b"Hello!".to_vec())]);
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::codes::code93::Code93Meta;
    use anyd::output::LinearBuf;
    let all: Vec<u8> = (0u8..128).collect();
    let enc = Code93Encoder::new();
    for (data, full_ascii) in [
        (&b"TEST93"[..], false),
        (b"", false),
        (b"ABC-123 $/+%.", false),
        (b"", true),
        (b"TEST93", true),
        (b"Hello, World!", true),
        (b"a\tb\nc", true),
        (&all[..], true),
    ] {
        let symbol = enc.build(data, full_ascii).unwrap();
        let Encoding::Linear(expected) = enc.encode(&symbol).unwrap() else {
            panic!("Code 93 encodes to a linear pattern");
        };
        let meta = Code93Meta { full_ascii };
        let mut storage = [0u8; 256];
        let mut buf = LinearBuf::new(&mut storage);
        enc.encode_into(data, &meta, &mut buf).unwrap();
        assert_eq!(buf, expected);
        assert!(buf.len() <= Code93Encoder::max_modules(data.len(), &meta));
        if !full_ascii {
            assert_eq!(buf.len(), Code93Encoder::max_modules(data.len(), &meta));
        }
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 2];
        let err = enc.encode_into(data, &meta, &mut LinearBuf::new(&mut tiny));
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }
    // Invalid input is rejected before anything is written.
    let mut storage = [0u8; 256];
    let mut buf = LinearBuf::new(&mut storage);
    let base = Code93Meta { full_ascii: false };
    assert!(enc.encode_into(b"ABc", &base, &mut buf).is_err());
    let fa = Code93Meta { full_ascii: true };
    assert!(enc.encode_into(b"AB\x80", &fa, &mut buf).is_err());
    assert!(buf.is_empty());
}
