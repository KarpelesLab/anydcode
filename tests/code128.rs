//! Integration tests for Code 128 / GS1-128: an independent reference vector plus
//! encode → decode → re-encode identity across all three code sets, set switches,
//! shifts, and a GS1-128 Application Identifier example.
#![cfg(all(feature = "decode", feature = "encode", feature = "code128"))]

use anyd::codes::code128::{Code128Decoder, Code128Encoder, Code128Input};
use anyd::output::Encoding;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn data(text: &str) -> Vec<Code128Input> {
    text.bytes().map(Code128Input::Data).collect()
}

/// Modules of an encoded symbol, as a run-length bit string for readable diffs.
fn modules(enc: &Encoding) -> Vec<bool> {
    match enc {
        Encoding::Linear(p) => p.modules.clone(),
        Encoding::Matrix(_) => panic!("expected linear"),
    }
}

/// Encode → decode → re-encode must reproduce the identical module pattern, and the
/// decoded meta must round-trip through encode unchanged.
fn assert_roundtrip(input: &[Code128Input], gs1: bool) {
    let encoder = Code128Encoder::new();
    let built = if gs1 {
        encoder.build_gs1(input).unwrap()
    } else {
        encoder.build(input).unwrap()
    };
    let encoding = encoder.encode(&built).unwrap();

    let decoded = Code128Decoder::new().decode(&encoding).unwrap();
    // Decoded symbol re-encodes byte-for-byte identically.
    let reencoded = encoder.encode(&decoded).unwrap();
    assert_eq!(encoding, reencoded, "re-encode differs from original");

    // The decoded meta equals the built meta (exact symbol-value sequence preserved).
    assert_eq!(decoded.meta, built.meta, "meta not preserved");
    assert_eq!(decoded.symbology, built.symbology);
}

/// Independent reference: Wikipedia "Code 128" worked example. Encoding "PJJ123C"
/// under Start Code A yields symbol values [103,48,42,42,17,18,19,35] and a
/// modulo-103 check character of 54. We pin that exact sequence in meta and check the
/// rendered module row against the concatenated documented patterns.
#[test]
fn wikipedia_reference_vector() {
    use anyd::Symbol;
    use anyd::codes::code128::Code128Meta;

    let meta = Code128Meta {
        gs1: false,
        symbols: vec![103, 48, 42, 42, 17, 18, 19, 35],
    };
    let symbol = Symbol::new(Symbology::Code128, Vec::new(), SymbolMeta::Code128(meta));
    let encoding = Code128Encoder::new().encode(&symbol).unwrap();

    // Concatenate the documented width patterns (Start A .. C, check 54, Stop).
    let mut expected = Vec::new();
    for widths in [
        "211412", "313121", "112133", "112133", "123221", "223211", "221132", "131321",
        "311123",  // check character value 54
        "2331112", // Stop
    ] {
        let mut bar = true;
        for w in widths.bytes().map(|b| (b - b'0') as usize) {
            expected.extend(std::iter::repeat_n(bar, w));
            bar = !bar;
        }
    }
    assert_eq!(modules(&encoding), expected);

    // And it decodes back to the same sequence.
    let decoded = Code128Decoder::new().decode(&encoding).unwrap();
    if let SymbolMeta::Code128(m) = &decoded.meta {
        assert_eq!(m.symbols, vec![103, 48, 42, 42, 17, 18, 19, 35]);
    } else {
        panic!("expected Code128 meta");
    }
    assert_eq!(decoded.text().as_deref(), Some("PJJ123C"));
}

#[test]
fn roundtrip_code_b_text() {
    let input = data("Hello, World!");
    assert_roundtrip(&input, false);
    let built = Code128Encoder::new().build(&input).unwrap();
    assert_eq!(built.text().as_deref(), Some("Hello, World!"));
    assert_eq!(built.symbology, Symbology::Code128);
}

#[test]
fn roundtrip_code_c_all_digits() {
    // Long even digit run should latch into Code C.
    let input = data("00112233445566778899");
    assert_roundtrip(&input, false);
    let built = Code128Encoder::new().build(&input).unwrap();
    assert_eq!(built.text().as_deref(), Some("00112233445566778899"));
    // Start C then ten digit pairs = 11 symbol values.
    if let SymbolMeta::Code128(m) = &built.meta {
        assert_eq!(m.symbols[0], 105); // Start C
        assert_eq!(m.symbols.len(), 11);
    } else {
        panic!();
    }
}

#[test]
fn roundtrip_mixed_switches() {
    // Text, then a long digit run (latch to C), then text again (latch back).
    assert_roundtrip(&data("ABC1234567890XYZ"), false);
    // Odd-length digit run inside text exercises the pre-emit-one-digit path.
    assert_roundtrip(&data("X1234567Y"), false);
}

#[test]
fn roundtrip_code_a_controls_and_shift() {
    // A tab (0x09) forces Code A; surrounding lower-case forces Code B → Shift usage.
    let mut input = data("abc");
    input.push(Code128Input::Data(0x09)); // HT control, only in Code A
    input.extend(data("def"));
    assert_roundtrip(&input, false);

    // A run of control characters latches into Code A.
    let input: Vec<Code128Input> = (1u8..=10).map(Code128Input::Data).collect();
    assert_roundtrip(&input, false);
}

#[test]
fn roundtrip_gs1_ai_example() {
    // GS1-128: (01) GTIN-14 (fixed length) followed by (10) batch (variable) with an
    // FNC1 AI separator before a second variable AI (21) serial.
    let mut input = data("0109521234543213"); // AI (01) + 14-digit GTIN
    input.extend(data("10ABC123")); // AI (10) batch/lot
    input.push(Code128Input::Fnc1); // AI separator (10 is variable length)
    input.extend(data("21XYZ")); // AI (21) serial
    assert_roundtrip(&input, true);

    let built = Code128Encoder::new().build_gs1(&input).unwrap();
    assert_eq!(built.symbology, Symbology::Gs1_128);
    if let SymbolMeta::Code128(m) = &built.meta {
        assert!(m.gs1);
        assert_eq!(m.symbols[0], 105); // Start C (leading digits)
        assert_eq!(m.symbols[1], 102); // FNC1 in first data position
    } else {
        panic!();
    }
    // The decoded payload renders the AI separator FNC1 as the GS control byte.
    let bytes = built.payload_bytes();
    assert!(
        bytes.contains(&0x1D),
        "AI separator should decode to GS byte"
    );
}

#[test]
fn decode_rejects_bad_check_character() {
    let encoder = Code128Encoder::new();
    let built = encoder.build_text("TEST").unwrap();
    let mut encoding = encoder.encode(&built).unwrap();
    // Corrupt one module inside the data region to break the check character.
    if let Encoding::Linear(p) = &mut encoding {
        let mid = p.modules.len() / 2;
        p.modules[mid] = !p.modules[mid];
    }
    assert!(Code128Decoder::new().decode(&encoding).is_err());
}

#[test]
fn encode_rejects_non_ascii_byte() {
    let encoder = Code128Encoder::new();
    let input = vec![Code128Input::Data(200)];
    assert!(encoder.build(&input).is_err());
}

/// The heap-free `encode_into` path writes exactly the modules `Encode` returns.
#[test]
fn encode_into_matches_encode() {
    use anyd::output::LinearBuf;
    let encoder = Code128Encoder::new();
    let controls: Vec<Code128Input> = (1u8..=10).map(Code128Input::Data).collect();
    let mut shifted = data("abc");
    shifted.push(Code128Input::Data(0x09));
    shifted.extend(data("def"));
    let mut ais = data("0109521234543213");
    ais.extend(data("10ABC123"));
    ais.push(Code128Input::Fnc1);
    ais.extend(data("21XYZ"));
    let symbols = [
        encoder.build(&data("PJJ123C")).unwrap(),
        encoder.build(&data("Hello, World!")).unwrap(),
        encoder.build(&data("00112233445566778899")).unwrap(),
        encoder.build(&data("X1234567Y")).unwrap(),
        encoder.build(&controls).unwrap(),
        encoder.build(&shifted).unwrap(),
        encoder.build_gs1(&ais).unwrap(),
        encoder.build_gs1(&[]).unwrap(),
    ];
    for symbol in &symbols {
        let SymbolMeta::Code128(meta) = &symbol.meta else {
            panic!("expected Code128 meta");
        };
        let Encoding::Linear(expected) = encoder.encode(symbol).unwrap() else {
            panic!("Code 128 encodes to a linear pattern");
        };
        let mut storage = [0u8; 64];
        let mut buf = LinearBuf::new(&mut storage);
        encoder.encode_into(&meta.symbols, &mut buf).unwrap();
        assert_eq!(buf, expected);
        assert_eq!(buf.len(), Code128Encoder::max_modules(meta.symbols.len()));
        // Too-small storage reports a capacity error instead of panicking.
        let mut tiny = [0u8; 2];
        let err = encoder.encode_into(&meta.symbols, &mut LinearBuf::new(&mut tiny));
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }
    // Invalid sequences are rejected before anything is written.
    let mut storage = [0u8; 64];
    let mut buf = LinearBuf::new(&mut storage);
    assert!(encoder.encode_into(&[], &mut buf).is_err());
    assert!(encoder.encode_into(&[48, 42], &mut buf).is_err());
    assert!(encoder.encode_into(&[104, 106], &mut buf).is_err());
    assert!(buf.is_empty());
}

/// The heap-free planner writes exactly the symbol values `build`/`build_gs1` pin in
/// meta, within `max_symbols`, and reports a too-short buffer as a capacity error.
#[test]
fn plan_into_matches_build() {
    let encoder = Code128Encoder::new();

    let mut cases: Vec<(Vec<Code128Input>, bool)> = Vec::new();
    for text in [
        "PJJ123C",
        "Hello, World!",
        "00112233445566778899", // all digits, even: Start C
        "123",                  // odd all-digit
        "1234",                 // short even all-digit
        "12345",                // odd leading run >= 4
        "ABC1234567890XYZ",     // latch to C and back
        "X1234567Y",            // odd run inside text
        "X1234",                // run of 4 at end
        "X123Y",                // short run stays in B
        "abc\tdef",             // shift to A
        "\t\n\r",               // Start A, stays in A
        "\tab",                 // latch A -> B
        "12\tab",
        "1234\x01", // leave C into A
        "a",
        "0",
    ] {
        cases.push((data(text), false));
        cases.push((data(text), true));
    }
    let controls: Vec<Code128Input> = (1u8..=10).map(Code128Input::Data).collect();
    cases.push((controls, false));
    let mut ais = data("0109521234543213");
    ais.extend(data("10ABC123"));
    ais.push(Code128Input::Fnc1);
    ais.extend(data("21XYZ"));
    cases.push((ais.clone(), true));
    cases.push((ais, false));
    let mut sep = data("123");
    sep.push(Code128Input::Fnc1);
    sep.extend(data("4567"));
    sep.push(Code128Input::Fnc1);
    cases.push((sep.clone(), true));
    cases.push((sep, false));
    cases.push((vec![Code128Input::Fnc1], false));
    cases.push((Vec::new(), true)); // empty GS1
    // A deterministic pseudo-random sweep over digits, controls, text and FNC1.
    let mut seed = 0x1234_5678u32;
    for _ in 0..300 {
        let mut input = Vec::new();
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let len = (seed >> 16) as usize % 24;
        for _ in 0..len {
            seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
            let r = (seed >> 16) as u8;
            input.push(match r % 8 {
                0..=3 => Code128Input::Data(b'0' + r % 10),
                4 => Code128Input::Data(r % 32),
                5 => Code128Input::Fnc1,
                _ => Code128Input::Data(32 + (r >> 1) % 96),
            });
        }
        cases.push((input, seed & 1 == 0));
    }

    for (input, gs1) in &cases {
        let built = if *gs1 {
            encoder.build_gs1(input)
        } else {
            encoder.build(input)
        };
        let Ok(built) = built else {
            // Only an empty plain input is rejected here.
            assert!(input.is_empty() && !gs1);
            continue;
        };
        let SymbolMeta::Code128(meta) = &built.meta else {
            panic!("expected Code128 meta");
        };
        let mut out = [0u8; 64];
        let n = encoder.plan_into(input, *gs1, &mut out).unwrap();
        assert_eq!(&out[..n], &meta.symbols[..], "input {input:?} gs1 {gs1}");
        assert!(n <= Code128Encoder::max_symbols(input.len()));

        // Exactly-sized storage succeeds; one short is a capacity error.
        let mut exact = vec![0u8; n];
        assert_eq!(encoder.plan_into(input, *gs1, &mut exact), Ok(n));
        let mut short = vec![0u8; n - 1];
        let err = encoder.plan_into(input, *gs1, &mut short);
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }

    // Invalid input is rejected.
    let mut out = [0u8; 8];
    assert!(encoder.plan_into(&[], false, &mut out).is_err());
    assert!(
        encoder
            .plan_into(&[Code128Input::Data(200)], false, &mut out)
            .is_err()
    );
}

/// `encode_text_into` renders the same modules as `build_text` + `Encode`.
#[test]
fn encode_text_into_matches_build_text() {
    use anyd::output::LinearBuf;
    let encoder = Code128Encoder::new();
    for text in ["PJJ123C", "Hello, World!", "0123456789", "X12345Y", "a\tb"] {
        let Encoding::Linear(expected) =
            encoder.encode(&encoder.build_text(text).unwrap()).unwrap()
        else {
            panic!("Code 128 encodes to a linear pattern");
        };
        let mut symbols = [0u8; Code128Encoder::max_symbols(16)];
        let mut storage = [0u8; LinearBuf::bytes_for(Code128Encoder::max_modules(
            Code128Encoder::max_symbols(16),
        ))];
        let mut buf = LinearBuf::new(&mut storage);
        encoder
            .encode_text_into(text.as_bytes(), &mut symbols, &mut buf)
            .unwrap();
        assert_eq!(buf, expected);

        let mut tiny = [0u8; 1];
        let err = encoder.encode_text_into(text.as_bytes(), &mut tiny, &mut buf);
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    }
}
