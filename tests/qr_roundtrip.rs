//! End-to-end QR round-trip tests: encode → decode → re-encode must be identical,
//! across modes, EC levels and versions, and must survive correctable errors.
#![cfg(all(feature = "decode", feature = "encode", feature = "qr"))]

use anyd::codes::qr::{EcLevel, Mask, QrDecoder, QrEncoder, Version};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::traits::{Decode, Encode};

/// Assert that a symbol decodes to the same segments/meta and re-encodes identically.
fn assert_lossless(segments: Vec<Segment>, level: EcLevel) {
    let enc = QrEncoder::new();
    let symbol = enc.build(segments.clone(), level).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = QrDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ after decode"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs after decode");

    // The decoded symbol must re-encode to the exact same matrix.
    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn numeric_all_levels() {
    for level in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H] {
        assert_lossless(vec![Segment::numeric(b"0123456789012345".to_vec())], level);
    }
}

#[test]
fn alphanumeric_all_levels() {
    for level in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H] {
        assert_lossless(
            vec![Segment::alphanumeric(b"HELLO WORLD 123 $%*+-./:".to_vec())],
            level,
        );
    }
}

#[test]
fn byte_utf8() {
    assert_lossless(
        vec![Segment::byte("héllo wörld — 日本語".as_bytes().to_vec())],
        EcLevel::M,
    );
}

#[test]
fn mixed_segments_preserve_boundaries() {
    // A payload deliberately split across modes; the exact segmentation must survive.
    let segments = vec![
        Segment::numeric(b"12345".to_vec()),
        Segment::alphanumeric(b"ABC".to_vec()),
        Segment::byte(b"xyz!".to_vec()),
    ];
    assert_lossless(segments, EcLevel::Q);
}

#[test]
fn large_payload_forces_high_version() {
    // ~1000 bytes forces a version >= 7 (which carries version information blocks).
    let data = vec![b'A'; 1000];
    assert_lossless(vec![Segment::byte(data)], EcLevel::L);
}

#[test]
fn numeric_odd_remainders() {
    // Lengths that exercise the 1- and 2-digit trailing groups.
    assert_lossless(vec![Segment::numeric(b"7".to_vec())], EcLevel::H);
    assert_lossless(vec![Segment::numeric(b"42".to_vec())], EcLevel::H);
    assert_lossless(vec![Segment::numeric(b"100".to_vec())], EcLevel::H);
}

#[test]
fn survives_correctable_errors() {
    // Corrupt a handful of modules and confirm error correction still recovers the
    // exact payload. EC level H tolerates ~30% damage.
    let enc = QrEncoder::new();
    let symbol = enc
        .build_text("RESILIENT PAYLOAD 2026", EcLevel::H)
        .unwrap();
    let Encoding::Matrix(mut matrix) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };

    // Flip a diagonal band of data modules (avoiding the finder corners).
    for k in 0..12 {
        let x = 9 + k;
        let y = 9 + k;
        if x < matrix.width() && y < matrix.height() {
            let v = matrix.get(x, y);
            matrix.set(x, y, !v);
        }
    }

    let decoded = QrDecoder::new().decode(&Encoding::Matrix(matrix)).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("RESILIENT PAYLOAD 2026"));
}

#[test]
fn text_convenience_roundtrip() {
    let enc = QrEncoder::new();
    let symbol = enc
        .build_text("https://github.com/KarpelesLab/anydcode", EcLevel::M)
        .unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = QrDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("https://github.com/KarpelesLab/anydcode")
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

// --- Automatic mode selection in build_text ---

/// Build from text, assert the chosen segmentation, and require a full
/// encode → decode → re-encode round trip.
fn assert_text_segments(text: &str, level: EcLevel, want: &[Segment]) {
    let enc = QrEncoder::new();
    let symbol = enc.build_text(text, level).unwrap();
    assert_eq!(
        symbol.segments, want,
        "unexpected segmentation for {text:?}"
    );
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = QrDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments);
    assert_eq!(decoded.text().as_deref(), Some(text));
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn build_text_picks_alphanumeric() {
    assert_text_segments(
        "HELLO WORLD",
        EcLevel::M,
        &[Segment::alphanumeric(b"HELLO WORLD".to_vec())],
    );
}

#[test]
fn build_text_picks_numeric() {
    assert_text_segments(
        "12345678901234567890",
        EcLevel::M,
        &[Segment::numeric(b"12345678901234567890".to_vec())],
    );
}

#[test]
fn build_text_splits_mixed_content() {
    assert_text_segments(
        "https://example.com/A012345678901234",
        EcLevel::M,
        &[
            Segment::byte(b"https://example.com/A".to_vec()),
            Segment::numeric(b"012345678901234".to_vec()),
        ],
    );
}

#[test]
fn build_text_keeps_utf8_in_byte_mode() {
    assert_text_segments(
        "héllo wörld",
        EcLevel::M,
        &[Segment::byte("héllo wörld".as_bytes().to_vec())],
    );
}

#[test]
fn build_text_never_worse_than_single_byte_segment() {
    use anyd::symbol::{Symbol, SymbolMeta};
    let version_of = |s: &Symbol| match &s.meta {
        SymbolMeta::Qr(m) => m.version.number(),
        _ => unreachable!(),
    };
    let enc = QrEncoder::new();
    let zeros = "0".repeat(500);
    for text in [
        "HELLO WORLD",
        zeros.as_str(),
        "WIFI:S:CAFE;T:WPA;P:12345678;;",
        "https://example.com/?id=1234567890123456789012345678901234567890",
        "mixed CASE with 123 numbers and symbols !@#",
    ] {
        let auto = enc.build_text(text, EcLevel::M).unwrap();
        let byte = enc
            .build(vec![Segment::byte(text.as_bytes().to_vec())], EcLevel::M)
            .unwrap();
        assert!(
            version_of(&auto) <= version_of(&byte),
            "auto segmentation chose a larger version for {text:?}"
        );
    }
}

/// The heap-free `encode_into` path reproduces `build` + `Encode` exactly, both with
/// pinned parameters and when choosing version and mask itself.
#[test]
fn encode_into_matches_encode() {
    use anyd::segment::SegmentView;
    use anyd::symbol::SymbolMeta;

    let enc = QrEncoder::new();
    let mut scratch = [0u8; QrEncoder::MAX_BUFFER_LEN];
    let mut storage = [0u8; QrEncoder::MAX_BUFFER_LEN];
    let long: String = (0..600).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    for text in [
        "",
        "HELLO WORLD",
        "0123456789",
        "https://example.com/?q=1",
        &long,
    ] {
        for level in [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H] {
            let symbol = enc.build_text(text, level).unwrap();
            let Encoding::Matrix(expected) = enc.encode(&symbol).unwrap() else {
                panic!("QR encodes to a matrix");
            };
            let SymbolMeta::Qr(meta) = symbol.meta else {
                panic!("QR meta")
            };
            let views: Vec<SegmentView<'_>> = symbol.segments.iter().map(|s| s.view()).collect();

            let (grid, pinned) = enc
                .encode_into(
                    &views,
                    level,
                    Some(meta.version),
                    Some(meta.mask),
                    &mut scratch,
                    &mut storage,
                )
                .unwrap();
            assert_eq!(grid, expected);
            assert_eq!(pinned, meta);

            let (grid, auto) = enc
                .encode_into(&views, level, None, None, &mut scratch, &mut storage)
                .unwrap();
            assert_eq!(grid, expected);
            assert_eq!(auto, meta);

            // Exactly-sized buffers work; one byte short is a capacity error.
            let len = QrEncoder::buffer_len(meta.version);
            let (s, t) = (&mut scratch[..len], &mut storage[..len]);
            assert!(enc.encode_into(&views, level, None, None, s, t).is_ok());
            let err = enc.encode_into(
                &views,
                level,
                None,
                None,
                &mut scratch[..len],
                &mut storage[..len - 1],
            );
            assert!(matches!(err, Err(anyd::Error::Capacity { .. })));
        }
    }

    // Single-mode convenience picks the densest mode.
    let (grid, meta) = enc
        .encode_text_into(b"0123456789", EcLevel::M, &mut scratch, &mut storage)
        .unwrap();
    let decoded = QrDecoder::new()
        .decode(&Encoding::Matrix(anyd::output::BitMatrix::from(&grid)))
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("0123456789"));
    assert_eq!(decoded.meta, SymbolMeta::Qr(meta));
    assert_eq!(decoded.modes(), vec![anyd::Mode::Numeric]);
}

/// Kanji mode: every Shift-JIS double byte the encoder accepts must decode back to
/// the same two bytes. Trail bytes below 0x40 used to be accepted and aliased onto
/// another character (e.g. 0x8200 encoded as 0x8240).
#[test]
fn kanji_is_injective_over_all_byte_pairs() {
    use anyd::segment::SegmentView;
    let enc = QrEncoder::new();
    let (version, mask) = (Version::new(1), Mask::new(0));
    let mut scratch = [0u8; QrEncoder::MAX_BUFFER_LEN];
    let mut storage = [0u8; QrEncoder::MAX_BUFFER_LEN];
    let mut accepted = 0;
    for hi in 0..=255u8 {
        for lo in 0..=255u8 {
            let pair = [hi, lo];
            let views = [SegmentView {
                mode: anyd::Mode::Kanji,
                data: &pair,
            }];
            let Ok((grid, _)) = enc.encode_into(
                &views,
                EcLevel::L,
                version,
                mask,
                &mut scratch,
                &mut storage,
            ) else {
                continue;
            };
            accepted += 1;
            let decoded = QrDecoder::new()
                .decode(&Encoding::Matrix(anyd::output::BitMatrix::from(&grid)))
                .unwrap();
            assert_eq!(decoded.segments, [Segment::kanji(pair.to_vec())]);
        }
    }
    // Every 13-bit value is reachable, so any decoded Kanji segment re-encodes.
    assert_eq!(accepted, 1 << 13);
    assert_lossless(
        vec![Segment::kanji(vec![0x93, 0x5F, 0xE4, 0xAA])],
        EcLevel::M,
    );
}
