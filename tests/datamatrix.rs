//! End-to-end Data Matrix round-trip tests: encode → decode → re-encode must be
//! byte-identical, across square sizes and ASCII / Base256 encodation, and must
//! survive correctable errors.
#![cfg(all(feature = "decode", feature = "encode", feature = "datamatrix"))]

use anyd::codes::datamatrix::{DataMatrixDecoder, DataMatrixEncoder};
use anyd::output::Encoding;
use anyd::traits::{Decode, Encode};

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
    use anyd::codes::datamatrix::Encodation;
    use anyd::symbol::SymbolMeta;
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

/// The heap-free `encode_into` / `encode_data_into` paths write exactly the modules
/// `Encode` returns, across sizes and encodations.
#[test]
fn encode_into_matches_encode() {
    use anyd::SegmentView;
    use anyd::codes::datamatrix::Encodation;
    use anyd::output::MatrixBuf;
    use anyd::symbol::SymbolMeta;

    let enc = DataMatrixEncoder::new();
    let mut payloads: Vec<Vec<u8>> = vec![
        b"".to_vec(),
        b"7".to_vec(),
        b"123456".to_vec(),
        b"HELLO WORLD".to_vec(),
        vec![b'A', 0x80, b'B'],
        (0u16..=255).map(|b| b as u8).collect(),
        (0..300).map(|i| (i * 7 % 256) as u8).collect(),
    ];
    for n in [20usize, 60, 120, 260, 600, 1000, 3116] {
        payloads.push((0..n).map(|i| b'0' + (i % 10) as u8).collect());
    }
    // Largest symbol filled with text (one codeword per byte).
    payloads.push((0..1558).map(|i| b'A' + (i % 26) as u8).collect());

    let mut scratch = [0u8; DataMatrixEncoder::MAX_SCRATCH_LEN];
    let mut storage = [0u8; DataMatrixEncoder::MAX_STORAGE_LEN];
    let check = |symbol: &anyd::Symbol, data: &[u8], forced: Option<usize>| {
        let mut scratch = [0u8; DataMatrixEncoder::MAX_SCRATCH_LEN];
        let mut storage = [0u8; DataMatrixEncoder::MAX_STORAGE_LEN];
        let Encoding::Matrix(expected) = enc.encode(symbol).unwrap() else {
            panic!("Data Matrix encodes to a matrix");
        };
        let SymbolMeta::DataMatrix(meta) = &symbol.meta else {
            panic!("expected Data Matrix meta");
        };
        let size = meta.symbol_size;
        let views: Vec<SegmentView> = symbol.segments.iter().map(|s| s.view()).collect();

        // Exactly-sized buffers suffice.
        let mut sc = vec![0u8; DataMatrixEncoder::scratch_len(size).unwrap()];
        let mut st = vec![0u8; DataMatrixEncoder::storage_len(size).unwrap()];
        assert!(sc.len() <= DataMatrixEncoder::MAX_SCRATCH_LEN);
        assert_eq!(st.len(), MatrixBuf::bytes_for(size, size));
        let buf = enc
            .encode_into(&views, &meta.encodations, size, &mut sc, &mut st)
            .unwrap();
        assert_eq!(buf, expected, "encode_into, size {size}");

        let buf = enc
            .encode_data_into(data, forced, &mut scratch, &mut storage)
            .unwrap();
        assert_eq!(buf, expected, "encode_data_into, size {size}");

        // One byte short of either buffer is a capacity error, not a panic.
        let short = sc.len() - 1;
        let err = enc.encode_into(
            &views,
            &meta.encodations,
            size,
            &mut sc[..short],
            &mut storage,
        );
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        let err = enc.encode_data_into(data, forced, &mut sc[..short], &mut storage);
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        let short = st.len() - 1;
        let err = enc.encode_into(
            &views,
            &meta.encodations,
            size,
            &mut scratch,
            &mut st[..short],
        );
        assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    };

    for data in &payloads {
        let symbol = enc.build(data).unwrap();
        check(&symbol, data, None);
        // Forced larger sizes exercise padding and multi-block interleaving.
        for size in [26usize, 52, 72, 104, 144] {
            if let Ok(symbol) = enc.build_sized(data, size) {
                check(&symbol, data, Some(size));
            }
        }
    }

    // Every square size, with a mixed ASCII/Base256 payload.
    let mut data = b"12AB".to_vec();
    data.extend_from_slice(&[0xC3, 0xA9]);
    for size in [
        10usize, 12, 14, 16, 18, 20, 22, 24, 26, 32, 36, 40, 44, 48, 52, 64, 72, 80, 88, 96, 104,
        120, 132, 144,
    ] {
        match enc.build_sized(&data, size) {
            Ok(symbol) => check(&symbol, &data, Some(size)),
            Err(e) => assert!(matches!(e, anyd::Error::Capacity { .. }), "size {size}"),
        }
        // A single ASCII segment with upper shifts (not the canonical split).
        let segs = [SegmentView::byte(&data)];
        let r = enc.encode_into(
            &segs,
            &[Encodation::Ascii],
            size,
            &mut scratch,
            &mut storage,
        );
        if size >= 14 {
            assert_eq!(r.unwrap().width(), size);
        } else {
            assert!(matches!(r, Err(anyd::Error::Capacity { .. })));
        }
    }

    // Oversized data, bad sizes, mismatched encodations and non-byte segments.
    let too_big = vec![b'A'; 1559];
    let err = enc.encode_data_into(&too_big, None, &mut scratch, &mut storage);
    assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    let err = enc.encode_data_into(b"HELLO", Some(11), &mut scratch, &mut storage);
    assert!(matches!(err, Err(anyd::Error::InvalidParameter { .. })));
    assert_eq!(DataMatrixEncoder::scratch_len(11), None);
    let err = enc.encode_into(
        &[SegmentView::byte(b"A")],
        &[],
        10,
        &mut scratch,
        &mut storage,
    );
    assert!(matches!(err, Err(anyd::Error::InvalidParameter { .. })));
    let err = enc.encode_into(
        &[SegmentView::numeric(b"1")],
        &[Encodation::Ascii],
        10,
        &mut scratch,
        &mut storage,
    );
    assert!(matches!(err, Err(anyd::Error::Unsupported { .. })));
    // Auto-size with tiny buffers.
    let err = enc.encode_data_into(b"HELLO WORLD", None, &mut [0u8; 4], &mut storage);
    assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
    let mut tiny = [0u8; 4];
    let err = enc.encode_data_into(b"HELLO WORLD", None, &mut scratch, &mut tiny);
    assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
}
