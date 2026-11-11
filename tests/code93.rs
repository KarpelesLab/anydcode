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

/// The symbol closes with a single-module termination bar; a wider final bar is not
/// a Code 93 stop (and would not survive the re-encode round trip).
#[test]
fn termination_bar_must_be_one_module() {
    let enc = Code93Encoder::new();
    let Encoding::Linear(mut pattern) = enc.encode(&enc.build(b"TEST93", false).unwrap()).unwrap()
    else {
        panic!("Code 93 encodes to a linear pattern");
    };
    assert!(
        Code93Decoder::new()
            .decode(&Encoding::Linear(pattern.clone()))
            .is_ok()
    );
    pattern.modules.extend([true, true]);
    assert!(
        Code93Decoder::new()
            .decode(&Encoding::Linear(pattern))
            .is_err()
    );
}

/// The 43 base characters in value order; values 43–46 are the shifts `($) (%) (/) (+)`.
const BASE: &[u8; 43] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ-. $/+%";
const SHIFT_DOLLAR: u8 = 43;
const SHIFT_PERCENT: u8 = 44;
const SHIFT_SLASH: u8 = 45;
const SHIFT_PLUS: u8 = 46;

/// Assemble a Code 93 row from raw symbol values, computing the C and K check
/// characters here (weights 1..=20 and 1..=15 from the right, modulo 47) rather than
/// through the encoder, which cannot be asked for an arbitrary value sequence.
fn row_of(values: &[u8]) -> Encoding {
    let enc = Code93Encoder::new();
    // The nine modules after the start character are the first data character.
    let first = |data: &[u8], full_ascii: bool| -> Vec<bool> {
        modules(&enc.encode(&enc.build(data, full_ascii).unwrap()).unwrap())[9..18].to_vec()
    };
    let pattern = |value: u8| -> Vec<bool> {
        match value {
            SHIFT_DOLLAR => first(&[0x01], true),  // SOH is ($)A
            SHIFT_PERCENT => first(&[0x1B], true), // ESC is (%)A
            SHIFT_SLASH => first(b"!", true),      // `!` is (/)A
            SHIFT_PLUS => first(b"a", true),       // `a` is (+)A
            v => first(&[BASE[v as usize]], false),
        }
    };
    let check = |values: &[u8], max_weight: usize| -> u8 {
        let sum: usize = values
            .iter()
            .rev()
            .enumerate()
            .map(|(i, &v)| (i % max_weight + 1) * v as usize)
            .sum();
        (sum % 47) as u8
    };
    let mut all = values.to_vec();
    all.push(check(&all, 20));
    all.push(check(&all, 15));

    let framing = modules(&enc.encode(&enc.build(b"", false).unwrap()).unwrap());
    let mut pattern_row = anyd::output::LinearPattern::new();
    pattern_row.quiet_zone = 10; // as the encoder records it
    pattern_row.modules.extend(&framing[..9]); // start
    for &v in &all {
        pattern_row.modules.extend(pattern(v));
    }
    pattern_row.modules.extend(&framing[framing.len() - 10..]); // stop + termination bar
    Encoding::Linear(pattern_row)
}

/// The hand-assembled rows agree with the encoder, check characters included.
#[test]
fn hand_assembled_rows_match_the_encoder() {
    let enc = Code93Encoder::new();
    let value = |c: u8| BASE.iter().position(|&b| b == c).unwrap() as u8;
    let test93: Vec<u8> = b"TEST93".iter().map(|&c| value(c)).collect();
    assert_eq!(
        row_of(&test93),
        enc.encode(&enc.build(b"TEST93", false).unwrap()).unwrap()
    );
    // A payload long enough to wrap both weight cycles.
    let long = b"CODE 93 WRAPS ITS WEIGHTS AFTER 20 AND 15";
    let values: Vec<u8> = long.iter().map(|&c| value(c)).collect();
    assert_eq!(
        row_of(&values),
        enc.encode(&enc.build(long, false).unwrap()).unwrap()
    );
    assert_eq!(
        row_of(&[SHIFT_PLUS, value(b'A'), value(b'B')]),
        enc.encode(&enc.build(b"aB", true).unwrap()).unwrap()
    );
}

/// A shift character must be followed by a letter it pairs with. Rows whose check
/// characters are valid but whose shifts are not are rejected — a shift followed by
/// a digit or punctuation used to underflow in the pair lookup and panic.
#[test]
fn broken_shift_pairs_are_rejected() {
    let value = |c: u8| BASE.iter().position(|&b| b == c).unwrap() as u8;
    for values in [
        vec![SHIFT_DOLLAR, value(b'1')],
        vec![SHIFT_DOLLAR, value(b'-')],
        vec![SHIFT_PLUS, value(b'0')],
        vec![SHIFT_PLUS, value(b'%')],
        vec![SHIFT_PERCENT, value(b'X')],
        vec![SHIFT_PERCENT, value(b'5')],
        vec![SHIFT_SLASH, value(b'M')],
        vec![SHIFT_SLASH, value(b' ')],
        vec![SHIFT_DOLLAR, SHIFT_DOLLAR, value(b'A')],
        vec![value(b'A'), SHIFT_PLUS],
    ] {
        assert!(
            matches!(
                Code93Decoder::new().decode(&row_of(&values)),
                Err(anyd::Error::Undecodable { .. })
            ),
            "{values:?} accepted"
        );
    }
    // Whereas a proper pair in the same position reads fine.
    let decoded = Code93Decoder::new()
        .decode(&row_of(&[SHIFT_DOLLAR, value(b'A'), value(b'1')]))
        .unwrap();
    assert_eq!(decoded.segments, vec![Segment::byte(vec![0x01, b'1'])]);
}
