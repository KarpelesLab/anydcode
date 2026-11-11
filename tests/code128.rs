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

/// Decode a raw symbol-value sequence (Start + data) rendered by `encode_into`.
fn decode_values(symbols: &[u8]) -> anyd::Symbol {
    let mut pattern = anyd::output::LinearPattern::new();
    Code128Encoder::new()
        .encode_into(symbols, &mut pattern)
        .unwrap();
    let encoding = Encoding::Linear(pattern);
    let decoded = Code128Decoder::new().decode(&encoding).unwrap();
    // Whatever the payload interpretation, the symbol values re-render identically.
    assert_eq!(Code128Encoder::new().encode(&decoded).unwrap(), encoding);
    decoded
}

/// ISO/IEC 15417 extended ASCII: a single FNC4 adds 128 to the next data character,
/// two in a row latch that until the next pair, and a single FNC4 inside a latched
/// run drops one character back to plain ASCII. Third-party encoders (zint, ...) use
/// this for Latin-1 text; the payload used to come back without the offset.
#[test]
fn fnc4_extended_ascii_payload() {
    const START_A: u8 = 103;
    const START_B: u8 = 104;
    const FNC4_A: u8 = 101;
    const FNC4_B: u8 = 100;
    // Code B: `i` is value 73, `A` 33, `B` 34.
    let shifted = decode_values(&[START_B, 33, FNC4_B, 73, 34]);
    assert_eq!(shifted.payload_bytes(), [b'A', 0xE9, b'B']);

    let latched = decode_values(&[
        START_B, FNC4_B, FNC4_B, 73, 73, FNC4_B, 33, 73, FNC4_B, FNC4_B, 34,
    ]);
    assert_eq!(latched.payload_bytes(), [0xE9, 0xE9, b'A', 0xE9, b'B']);

    // Code A: FNC4 is value 101; NUL is value 64, so FNC4 NUL is 0x80.
    let code_a = decode_values(&[START_A, FNC4_A, 64, 33]);
    assert_eq!(code_a.payload_bytes(), [0x80, b'A']);
}

/// Symbol-value sequences assembled by hand from the ISO/IEC 15417 code-set table
/// (set A: values 0–63 are ASCII 32–95 and 64–95 the controls 0–31; set B: values
/// 0–95 are ASCII 32–127; set C: the value is a digit pair), with the payload each
/// must decode to.
#[test]
fn code_set_semantics() {
    const SHIFT: u8 = 98;
    const CODE_C: u8 = 99;
    const CODE_B: u8 = 100;
    const CODE_A: u8 = 101;
    const FNC1: u8 = 102;
    let payload = |symbols: &[u8]| decode_values(symbols).payload_bytes();

    assert_eq!(payload(&[105, 0, 12, 99]), b"001299");
    assert_eq!(
        payload(&[103, 0, 33, 63, 64, 73, 95]),
        [b' ', b'A', b'_', 0, b'\t', 31]
    );
    assert_eq!(payload(&[104, 0, 33, 65, 95]), [b' ', b'A', b'a', 127]);

    // Shift borrows the other of A/B for exactly one character.
    assert_eq!(payload(&[104, 65, SHIFT, 65, 65]), [b'a', 1, b'a']);
    assert_eq!(payload(&[103, 33, SHIFT, 65, 33]), [b'A', b'a', b'A']);
    // Latches: C → B → A → C.
    assert_eq!(
        payload(&[105, 12, CODE_B, 33, CODE_A, 73, CODE_C, 34]),
        b"12A\t34"
    );
    // FNC2 (97) and FNC3 (96) carry no payload.
    assert_eq!(payload(&[104, 96, 97, 33]), b"A");

    // FNC1 straight after the Start marks GS1-128 and is not data; later ones are
    // field separators, reported as GS.
    let gs1 = decode_values(&[105, FNC1, 12, FNC1, 34]);
    assert_eq!(gs1.symbology, Symbology::Gs1_128);
    assert_eq!(gs1.payload_bytes(), b"12\x1d34");
    let plain = decode_values(&[104, 33, FNC1, 34]);
    assert_eq!(plain.symbology, Symbology::Code128);
    assert_eq!(plain.payload_bytes(), b"A\x1dB");
}

/// The planner's choices for inputs whose shortest encoding is unambiguous.
#[test]
fn planner_reference_sequences() {
    let encoder = Code128Encoder::new();
    let planned = |symbol: anyd::Symbol| match symbol.meta {
        SymbolMeta::Code128(meta) => meta.symbols,
        _ => panic!("Code 128 meta"),
    };
    // GS1-128 (01)95012345678903: Start C, FNC1, then eight digit pairs.
    assert_eq!(
        planned(encoder.build_gs1(&data("0195012345678903")).unwrap()),
        [105, 102, 1, 95, 1, 23, 45, 67, 89, 3]
    );
    // An even run of four or more trailing digits moves to set C.
    assert_eq!(
        planned(encoder.build_text("ABC123456").unwrap()),
        [104, 33, 34, 35, 99, 12, 34, 56]
    );
    // Two digits are not worth a latch.
    assert_eq!(
        planned(encoder.build_text("A12").unwrap()),
        [104, 33, 17, 18]
    );
    // A leading control character starts in set A; an isolated one is shifted.
    assert_eq!(planned(encoder.build_text("\tA").unwrap()), [103, 73, 33]);
    assert_eq!(
        planned(encoder.build_text("a\tb").unwrap()),
        [104, 65, 98, 73, 66]
    );
    // Lower case after controls needs set B for good: latch, not shift.
    assert_eq!(
        planned(encoder.build_text("\t\tab").unwrap()),
        [103, 73, 73, 100, 65, 66]
    );
}

/// Whatever mix of ASCII, digit runs, controls and FNC1 goes in, the planned symbol
/// decodes to exactly that payload, fits the documented bound and round-trips.
#[test]
fn planner_random_inputs_roundtrip() {
    let mut rng = anyd::transform::Rng::new(0x0128);
    let mut below = |n: u64| (rng.next_u64() >> 20) % n;
    let encoder = Code128Encoder::new();
    for round in 0..6000 {
        let len = below(24) as usize;
        let style = below(4);
        let input: Vec<Code128Input> = (0..len)
            .map(|_| {
                let digit = Code128Input::Data(b'0' + below(10) as u8);
                match (style, below(7)) {
                    (0, _) => Code128Input::Data(below(128) as u8),
                    (1, _) => digit,
                    (2, 0) => Code128Input::Data(below(128) as u8),
                    (2, _) => digit,
                    (_, 0) => Code128Input::Fnc1,
                    (_, 1 | 2) => Code128Input::Data(below(32) as u8),
                    (_, 3 | 4) => Code128Input::Data(96 + below(32) as u8),
                    _ => digit,
                }
            })
            .collect();
        let gs1 = round % 2 == 0;
        let built = if gs1 {
            encoder.build_gs1(&input)
        } else {
            encoder.build(&input)
        };
        if input.is_empty() && !gs1 {
            assert!(matches!(built, Err(anyd::Error::InvalidData { .. })));
            continue;
        }
        let built = built.unwrap();
        let SymbolMeta::Code128(meta) = &built.meta else {
            panic!("Code 128 meta");
        };
        assert!(meta.symbols.len() <= Code128Encoder::max_symbols(input.len()));

        let mut expected: Vec<u8> = input
            .iter()
            .map(|el| match el {
                Code128Input::Data(b) => *b,
                Code128Input::Fnc1 => 0x1D,
            })
            .collect();
        // A plain symbol that opens with FNC1 *is* a GS1-128 symbol.
        let leading_fnc1 = !gs1 && input.first() == Some(&Code128Input::Fnc1);
        if leading_fnc1 {
            expected.remove(0);
        }
        assert_eq!(built.payload_bytes(), expected, "{input:?}");
        assert_eq!(meta.gs1, gs1 || leading_fnc1);

        let encoding = encoder.encode(&built).unwrap();
        let decoded = Code128Decoder::new().decode(&encoding).unwrap();
        assert_eq!(decoded, built, "{input:?}");
        assert_eq!(encoder.encode(&decoded).unwrap(), encoding);
    }
}

#[test]
fn encoder_rejects_malformed_sequences() {
    use anyd::Symbol;
    use anyd::codes::code128::Code128Meta;
    use anyd::output::LinearPattern;
    let encoder = Code128Encoder::new();
    let invalid_parameter = |symbols: &[u8]| {
        let mut out = LinearPattern::new();
        let err = encoder.encode_into(symbols, &mut out);
        // Nothing is written before the sequence is known to be valid.
        assert!(out.modules.is_empty());
        matches!(err, Err(anyd::Error::InvalidParameter { .. }))
    };
    assert!(invalid_parameter(&[])); // no Start
    assert!(invalid_parameter(&[33, 34])); // does not begin with a Start
    assert!(invalid_parameter(&[104, 106])); // Stop as data
    assert!(invalid_parameter(&[104, 33, 200])); // not a symbol value

    let symbols = vec![104, 33];
    let good = Symbol::new(
        Symbology::Code128,
        Vec::new(),
        SymbolMeta::Code128(Code128Meta {
            gs1: false,
            symbols,
        }),
    );
    encoder.encode(&good).unwrap();
    let mut wrong_symbology = good.clone();
    wrong_symbology.symbology = Symbology::Code39;
    let mut wrong_meta = good.clone();
    wrong_meta.meta = SymbolMeta::Itf(Default::default());
    for bad in [wrong_symbology, wrong_meta] {
        assert!(matches!(
            encoder.encode(&bad),
            Err(anyd::Error::InvalidParameter { .. })
        ));
    }

    assert!(matches!(
        encoder.build(&[]),
        Err(anyd::Error::InvalidData { .. })
    ));
    assert!(matches!(
        encoder.build_gs1(&[Code128Input::Data(128)]),
        Err(anyd::Error::InvalidData { .. })
    ));
    // GS1 mode with no data is just Start + FNC1, and still round-trips.
    assert_roundtrip(&[], true);
}

#[test]
fn decoder_rejects_malformed_rows() {
    use anyd::output::{BitMatrix, LinearPattern};
    let decoder = Code128Decoder::new();
    let row = |modules: Vec<bool>| {
        let mut pattern = LinearPattern::new();
        pattern.modules = modules;
        Encoding::Linear(pattern)
    };
    let undecodable =
        |e: &Encoding| matches!(decoder.decode(e), Err(anyd::Error::Undecodable { .. }));

    assert!(matches!(
        decoder.decode(&Encoding::Matrix(BitMatrix::new(35, 1, 0))),
        Err(anyd::Error::Unsupported { .. })
    ));
    assert!(undecodable(&row(Vec::new())));
    assert!(undecodable(&row(vec![true; 60])));
    assert!(undecodable(&row(vec![false; 60])));

    let encoder = Code128Encoder::new();
    let good = modules(
        &encoder
            .encode(&encoder.build_text("Ab\t1234").unwrap())
            .unwrap(),
    );
    decoder.decode(&row(good.clone())).unwrap();
    // The structural decoder takes the row cropped to its bars.
    let mut padded = vec![false];
    padded.extend(&good);
    assert!(undecodable(&row(padded)));
    for len in 0..good.len() {
        assert!(
            undecodable(&row(good[..len].to_vec())),
            "truncated to {len}"
        );
    }
    for at in 0..good.len() {
        let mut flipped = good.clone();
        flipped[at] = !flipped[at];
        assert!(undecodable(&row(flipped)), "module {at} flipped");
    }

    // Well-formed characters with a valid check, but not a legal sequence.
    for symbols in [
        &[104u8, 103][..], // a second Start as data
        &[105, 103],
        &[103, 105, 33],
        &[105, 12, 104],
    ] {
        let mut pattern = LinearPattern::new();
        encoder.encode_into(symbols, &mut pattern).unwrap();
        assert!(undecodable(&Encoding::Linear(pattern)), "{symbols:?}");
    }
}
