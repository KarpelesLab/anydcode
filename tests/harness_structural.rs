//! Tier A: generic *structural* round-trip harness.
//!
//! For every symbology this exercises the pure module-level loop — encode a
//! [`Symbol`] to an [`Encoding`], decode the `Encoding` back to a `Symbol`, and assert
//! three invariants:
//!
//! 1. the decoded segments equal the originals (lossless payload + segmentation),
//! 2. the decoded [`SymbolMeta`] equals the original (version/EC/mask preserved), and
//! 3. re-encoding the decoded symbol reproduces the byte-identical `Encoding`.
//!
//! No pixels are involved here; this is the clean-data contract that image sampling
//! (Tier B, `harness_image.rs`) must ultimately reproduce.
//!
//! ## Adding another symbology
//!
//! Everything is generic over the [`StructuralCodec`] trait. When a new symbology
//! lands, implement `StructuralCodec` for a wrapper around its encoder/decoder and add
//! one `#[test]` that feeds representative [`Symbol`]s through [`assert_roundtrip`].
//! Only QR is implemented at branch time, so only the QR codec appears below.

use anydcode::Symbol;
use anydcode::codes::qr::{EcLevel, QrDecoder, QrEncoder};
use anydcode::error::Result;
use anydcode::output::Encoding;
use anydcode::segment::Segment;
use anydcode::traits::{Decode, Encode};

/// A symbology whose encode/decode pair can be round-tripped at the module level.
trait StructuralCodec {
    /// Human-readable label used in assertion messages.
    fn name(&self) -> &'static str;
    /// Encode a symbol to module geometry.
    fn encode(&self, symbol: &Symbol) -> Result<Encoding>;
    /// Decode module geometry back to a symbol.
    fn decode(&self, encoding: &Encoding) -> Result<Symbol>;
}

/// The full structural contract for one symbol under one codec.
fn assert_roundtrip(codec: &dyn StructuralCodec, symbol: &Symbol) {
    let encoding = codec
        .encode(symbol)
        .unwrap_or_else(|e| panic!("[{}] encode failed: {e}", codec.name()));

    let decoded = codec
        .decode(&encoding)
        .unwrap_or_else(|e| panic!("[{}] decode failed: {e}", codec.name()));

    assert_eq!(
        decoded.segments,
        symbol.segments,
        "[{}] segments changed across round-trip",
        codec.name()
    );
    assert_eq!(
        decoded.meta,
        symbol.meta,
        "[{}] meta changed across round-trip",
        codec.name()
    );
    assert_eq!(
        decoded.payload_bytes(),
        symbol.payload_bytes(),
        "[{}] payload bytes changed across round-trip",
        codec.name()
    );

    let reencoded = codec
        .encode(&decoded)
        .unwrap_or_else(|e| panic!("[{}] re-encode failed: {e}", codec.name()));
    assert_eq!(
        reencoded,
        encoding,
        "[{}] re-encoding the decoded symbol is not byte-identical",
        codec.name()
    );
}

// ===================================================================================
// QR codec adapter
// ===================================================================================

struct QrCodec;

impl StructuralCodec for QrCodec {
    fn name(&self) -> &'static str {
        "QR"
    }
    fn encode(&self, symbol: &Symbol) -> Result<Encoding> {
        QrEncoder::new().encode(symbol)
    }
    fn decode(&self, encoding: &Encoding) -> Result<Symbol> {
        QrDecoder::new().decode(encoding)
    }
}

/// Build a QR symbol from segments at a level, pinning version/mask via the encoder.
fn qr_symbol(segments: Vec<Segment>, level: EcLevel) -> Symbol {
    QrEncoder::new()
        .build(segments, level)
        .expect("QR build should succeed for in-capacity data")
}

const ALL_LEVELS: [EcLevel; 4] = [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H];

#[test]
fn qr_numeric_across_levels_and_lengths() {
    for level in ALL_LEVELS {
        for len in [1usize, 2, 3, 5, 16, 40, 120] {
            let digits: Vec<u8> = (0..len).map(|i| b'0' + (i % 10) as u8).collect();
            assert_roundtrip(&QrCodec, &qr_symbol(vec![Segment::numeric(digits)], level));
        }
    }
}

#[test]
fn qr_alphanumeric_across_levels() {
    let data = b"HELLO WORLD 123 $%*+-./: ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    for level in ALL_LEVELS {
        for len in [1usize, 2, 7, 25, data.len()] {
            assert_roundtrip(
                &QrCodec,
                &qr_symbol(vec![Segment::alphanumeric(data[..len].to_vec())], level),
            );
        }
    }
}

#[test]
fn qr_byte_utf8_and_binary() {
    for level in ALL_LEVELS {
        assert_roundtrip(
            &QrCodec,
            &qr_symbol(
                vec![Segment::byte("héllo wörld — 日本語 🌍".as_bytes().to_vec())],
                level,
            ),
        );
        // Raw binary including NUL and high bytes.
        let bin: Vec<u8> = (0u16..=255).map(|b| b as u8).collect();
        assert_roundtrip(&QrCodec, &qr_symbol(vec![Segment::byte(bin)], level));
    }
}

#[test]
fn qr_kanji_segment() {
    // Shift-JIS bytes in the QR Kanji ranges.
    let kanji = vec![0x93u8, 0x5F, 0xE4, 0xAA];
    for level in ALL_LEVELS {
        assert_roundtrip(
            &QrCodec,
            &qr_symbol(vec![Segment::kanji(kanji.clone())], level),
        );
    }
}

#[test]
fn qr_mixed_segments_preserve_boundaries() {
    let segments = vec![
        Segment::numeric(b"12345".to_vec()),
        Segment::alphanumeric(b"ABC XYZ".to_vec()),
        Segment::byte(b"raw!\x00\xFF".to_vec()),
        Segment::numeric(b"6789".to_vec()),
    ];
    for level in ALL_LEVELS {
        assert_roundtrip(&QrCodec, &qr_symbol(segments.clone(), level));
    }
}

#[test]
fn qr_eci_switch() {
    let segments = vec![
        Segment::eci(26), // UTF-8
        Segment::byte("café".as_bytes().to_vec()),
    ];
    assert_roundtrip(&QrCodec, &qr_symbol(segments, EcLevel::M));
}

#[test]
fn qr_large_payload_high_version() {
    // ~1200 bytes forces a version >= 7 (which carries version-information blocks).
    assert_roundtrip(
        &QrCodec,
        &qr_symbol(vec![Segment::byte(vec![b'Z'; 1200])], EcLevel::L),
    );
}
