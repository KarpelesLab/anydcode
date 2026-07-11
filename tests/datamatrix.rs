//! End-to-end Data Matrix round-trip tests: encode → decode → re-encode must be
//! byte-identical, across square sizes and ASCII / Base256 encodation, and must
//! survive correctable errors.

use anydcode::codes::datamatrix::{DataMatrixDecoder, DataMatrixEncoder};
use anydcode::output::Encoding;
use anydcode::traits::{Decode, Encode};

/// Assert that a payload decodes to the same segments/meta and re-encodes identically.
fn assert_lossless(data: &[u8]) {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.payload_bytes(),
        data,
        "payload differs after decode"
    );
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ after decode"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs after decode");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn ascii_digits() {
    assert_lossless(b"123456");
    assert_lossless(b"1234567890");
    assert_lossless(b"7"); // lone digit -> ASCII char, not a pair
    assert_lossless(b"12345"); // odd digit count
}

#[test]
fn ascii_text() {
    assert_lossless(b"HELLO WORLD");
    assert_lossless(b"Data Matrix ECC 200!");
    assert_lossless(b"mix3d 12 t3xt 34 and digits 5678");
}

#[test]
fn base256_high_bytes() {
    assert_lossless(&[0x00, 0x80, 0xFF, 0xAB, 0xCD]);
    assert_lossless(&(0u16..=255).map(|b| b as u8).collect::<Vec<u8>>());
}

#[test]
fn mixed_ascii_and_base256() {
    // Alternating low/high byte runs exercise the encodation transitions.
    let mut data = b"ABC".to_vec();
    data.extend_from_slice(&[0x80, 0x90, 0xA0]);
    data.extend_from_slice(b"789012");
    data.extend_from_slice(&[0xFF]);
    assert_lossless(&data);
}

#[test]
fn segment_structure_is_recovered() {
    // "A" then a high byte then "B" must come back as ASCII / Base256 / ASCII.
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build(&[b'A', 0x80, b'B']).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    use anydcode::codes::datamatrix::Encodation;
    use anydcode::symbol::SymbolMeta;
    let SymbolMeta::DataMatrix(meta) = &decoded.meta else {
        panic!("expected Data Matrix meta");
    };
    assert_eq!(
        meta.encodations,
        vec![Encodation::Ascii, Encodation::Base256, Encodation::Ascii]
    );
    assert_eq!(decoded.segments.len(), 3);
}

#[test]
fn spans_many_sizes() {
    // Payloads of growing length force progressively larger square symbols.
    for n in [1usize, 5, 20, 60, 120, 260, 600, 1000] {
        let data: Vec<u8> = (0..n).map(|i| b'0' + (i % 10) as u8).collect();
        assert_lossless(&data);
    }
}

#[test]
fn forced_size_pads_correctly() {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build_sized(b"12", 26).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let Encoding::Matrix(m) = &encoding else {
        panic!("expected matrix");
    };
    assert_eq!((m.width(), m.height()), (26, 26));
    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), b"12");
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn survives_correctable_errors() {
    let enc = DataMatrixEncoder::new();
    // A 26x26 symbol has 28 EC codewords across one block: it corrects up to 14.
    let symbol = enc.build_sized(b"RESILIENT DATA MATRIX 2026", 26).unwrap();
    let Encoding::Matrix(mut matrix) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };

    // Flip a handful of interior modules (avoiding the finder/timing borders).
    for k in 0..6 {
        let x = 3 + k * 3;
        let y = 3 + k * 2;
        let v = matrix.get(x, y);
        matrix.set(x, y, !v);
    }

    let decoded = DataMatrixDecoder::new()
        .decode(&Encoding::Matrix(matrix))
        .unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("RESILIENT DATA MATRIX 2026")
    );
}

#[test]
fn text_convenience_roundtrip() {
    let enc = DataMatrixEncoder::new();
    let symbol = enc.build_text("https://example.com/anydcode").unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = DataMatrixDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("https://example.com/anydcode")
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}
