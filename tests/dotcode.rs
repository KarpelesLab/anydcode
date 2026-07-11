//! DotCode integration tests: an independent reference bitmap plus
//! encode→decode→re-encode round-trip identity across code sets and sizes.

use anyd::codes::dotcode::{DotCodeDecoder, DotCodeEncoder};
use anyd::output::Encoding;
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Render an [`Encoding::Matrix`] to `'0'`/`'1'` rows for comparison with the
/// documented dot bitmaps.
fn matrix_rows(encoding: &Encoding) -> Vec<String> {
    match encoding {
        Encoding::Matrix(m) => (0..m.height())
            .map(|y| {
                (0..m.width())
                    .map(|x| if m.get(x, y) { '1' } else { '0' })
                    .collect()
            })
            .collect(),
        Encoding::Linear(_) => panic!("DotCode must produce a matrix"),
    }
}

/// INDEPENDENT REFERENCE VECTOR.
///
/// AIM ISS DotCode Rev 4.0 Figure 6 (top-right), reproduced as zint
/// `backend/tests/test_dotcode.c` `test_encode` vector /*6*/ (auto mask → 1) for the
/// GS1 payload `[17]070620[10]ABC123456`. The bracketed AIs reduce to the byte
/// string `1707062010ABC123456` (AI 17 is fixed-length 6, AI 10 is last); this
/// reduction is itself cross-checked by zint dump vector /*24*/. Building through the
/// public API and rendering the matrix must reproduce the documented dots exactly.
#[test]
fn reference_bitmap_rev4_figure6() {
    let expected = [
        "10000000001010001000101",
        "01010101000100000101000",
        "00100010000000100000001",
        "01010001000001000001000",
        "10101010100000001010101",
        "00000100010100000100010",
        "00000000001010101010001",
        "00010001010001000001000",
        "00101010101000001010001",
        "01000100000001010000000",
        "10101000101000101000001",
        "00010101000100010101010",
        "10001000001010100000101",
        "01010001010001000001010",
        "10000010101010100010101",
        "01000101000101010101010",
    ];

    let encoder = DotCodeEncoder::new();
    let symbol = encoder.build_gs1_reduced(b"1707062010ABC123456").unwrap();
    let encoding = encoder.encode(&symbol).unwrap();
    assert_eq!(matrix_rows(&encoding), expected);
}

/// Encode → decode → re-encode identity, plus meta and payload recovery.
fn assert_roundtrip(symbol: &anyd::Symbol) {
    let encoder = DotCodeEncoder::new();
    let decoder = DotCodeDecoder::new();

    let encoding = encoder.encode(symbol).unwrap();
    let decoded = decoder.decode(&encoding).unwrap();

    // Re-encoding the decoded symbol reproduces the identical matrix.
    let reencoded = encoder.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoded matrix differs");

    // The pinned re-encode metadata survives the round-trip exactly.
    assert_eq!(decoded.symbology, Symbology::DotCode);
    match (&symbol.meta, &decoded.meta) {
        (SymbolMeta::DotCode(a), SymbolMeta::DotCode(b)) => assert_eq!(a, b, "meta differs"),
        _ => panic!("expected DotCodeMeta"),
    }
}

#[test]
fn roundtrip_across_code_sets_and_sizes() {
    let encoder = DotCodeEncoder::new();

    let cases: &[&[u8]] = &[
        b"A",
        b"AB",
        b"Hello, World!",
        b"ABCDE12345678",
        b"1234567890",
        b"1712345610",
        b"00",
        b"\x00ABCD1234567890",
        b"99aA[{00\x00",
        b"The quick brown fox jumps over the lazy dog 0123456789",
        b"lower case letters and DIGITS 42 mixed",
        b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        b"9999999999999999999999999999999999999999",
    ];
    for &data in cases {
        let symbol = encoder.build_bytes(data).unwrap();
        assert_roundtrip(&symbol);
        // Payload bytes must be recovered verbatim by the decoder.
        let decoded = DotCodeDecoder::new()
            .decode(&encoder.encode(&symbol).unwrap())
            .unwrap();
        assert_eq!(
            decoded.payload_bytes(),
            data,
            "payload mismatch for {data:?}"
        );
    }
}

#[test]
fn roundtrip_binary_and_high_bytes() {
    let encoder = DotCodeEncoder::new();
    let cases: Vec<Vec<u8>> = vec![
        vec![0x80, 0x81, 0x82, 0x83, 0x31, 0x32, 0x33, 0x34],
        b"abcde\x80\x81\x82\x83\x84\xff".to_vec(),
        (0u16..=255).map(|b| b as u8).collect(),
        vec![0xff; 40],
        b"text then binary \x80\x90\xa0\xb0 and more".to_vec(),
    ];
    for data in &cases {
        let symbol = encoder.build_bytes(data).unwrap();
        assert_roundtrip(&symbol);
        let decoded = DotCodeDecoder::new()
            .decode(&encoder.encode(&symbol).unwrap())
            .unwrap();
        assert_eq!(decoded.payload_bytes(), *data, "binary payload mismatch");
    }
}

#[test]
fn roundtrip_user_width() {
    let encoder = DotCodeEncoder::new();
    for width in [7usize, 13, 20, 30] {
        let symbol = encoder
            .build_bytes_width(b"WIDTH TEST 12345", width)
            .unwrap();
        assert_roundtrip(&symbol);
    }
}

#[test]
fn roundtrip_gs1_reduced() {
    let encoder = DotCodeEncoder::new();
    let symbol = encoder.build_gs1_reduced(b"1707062010ABC123456").unwrap();
    assert_roundtrip(&symbol);
}

#[test]
fn rejects_non_dotcode_symbol() {
    // A tampered matrix (all dark) should fail the Reed–Solomon check.
    let encoder = DotCodeEncoder::new();
    let symbol = encoder.build_bytes(b"CHECKSUM").unwrap();
    let mut encoding = encoder.encode(&symbol).unwrap();
    if let Encoding::Matrix(m) = &mut encoding {
        // Flip several data modules to force an RS mismatch.
        let w = m.width();
        for y in 0..m.height() {
            for x in 0..w {
                if (x + y) % 2 == 0 {
                    m.set(x, y, !m.get(x, y));
                }
            }
        }
    }
    assert!(DotCodeDecoder::new().decode(&encoding).is_err());
}
