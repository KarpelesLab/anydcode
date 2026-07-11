//! End-to-end QR round-trip tests: encode → decode → re-encode must be identical,
//! across modes, EC levels and versions, and must survive correctable errors.

use anyd::codes::qr::{EcLevel, QrDecoder, QrEncoder};
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
