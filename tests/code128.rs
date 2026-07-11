//! Integration tests for Code 128 / GS1-128: an independent reference vector plus
//! encode → decode → re-encode identity across all three code sets, set switches,
//! shifts, and a GS1-128 Application Identifier example.

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
