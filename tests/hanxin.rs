//! Han Xin Code (ISO/IEC 20830) integration tests.
//!
//! The headline test is an **independent reference vector**: the exact 23×23 module
//! bitmap that zint (`libzint`) emits for the numeric input "1234" at version 1, EC
//! level 4. It is reproduced verbatim from zint's own test suite
//! (`backend/tests/test_hanxin.c`, which cross-checks against ZXing-C++), decoded
//! here, and re-encoded to the identical bitmap. This pins the finder patterns,
//! numeric mode, GF(2^8)/`0x163` Reed–Solomon, picket-fence interleave, mask 2 and
//! the function information all at once against a third-party implementation.

use anyd::codes::hanxin::{EcLevel, HanXinDecoder, HanXinEncoder, HanXinMeta, Mask, Version};
use anyd::output::{BitMatrix, Encoding};
use anyd::segment::{Mode, Segment};
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// zint's Han Xin output for "1234", version 1, EC level 4 (23×23 modules).
const ZINT_1234_V1_L4: &str = "\
11111110101011001111111
10000000000000100000001
10111110111011101111101
10100000010111100000101
10101110011101101110101
10101110000101001110101
10101110111001101110101
00000000100000000000000
00010101101110000000000
01010110110001011011101
00100011000100100100101
01110111111110001000111
01001001110000001001000
00100101001111110010000
00000000011110110101000
00000000001011100000000
11111110110111101110101
00000010011010001110101
11111010101101001110101
00001010100111000000101
11101010111001101111101
11101010111111000000001
11101010000100101111111";

fn parse_bitmap(s: &str) -> BitMatrix {
    let rows: Vec<&str> = s.lines().collect();
    let size = rows.len();
    let mut m = BitMatrix::new(size, size, 3);
    for (y, row) in rows.iter().enumerate() {
        assert_eq!(row.len(), size, "row {y} wrong width");
        for (x, c) in row.bytes().enumerate() {
            if c == b'1' {
                m.set(x, y, true);
            }
        }
    }
    m
}

fn matrix_of(enc: &Encoding) -> &BitMatrix {
    match enc {
        Encoding::Matrix(m) => m,
        _ => panic!("expected a matrix encoding"),
    }
}

/// Decode zint's reference bitmap and check we recover the exact content and meta.
#[test]
fn zint_reference_decode() {
    let matrix = parse_bitmap(ZINT_1234_V1_L4);
    let symbol = HanXinDecoder::new().decode_matrix(&matrix).unwrap();

    assert_eq!(symbol.symbology, Symbology::HanXin);
    assert_eq!(symbol.segments, vec![Segment::numeric(b"1234".to_vec())]);
    assert_eq!(symbol.text().as_deref(), Some("1234"));

    let meta = match &symbol.meta {
        SymbolMeta::HanXin(m) => m,
        _ => panic!("expected HanXinMeta"),
    };
    assert_eq!(meta.version, Version::new(1).unwrap());
    assert_eq!(meta.ec_level, EcLevel::L4);
    assert_eq!(meta.mask, Mask::new(2).unwrap());
}

/// Re-encoding the decoded reference symbol reproduces zint's bitmap byte-for-byte.
#[test]
fn zint_reference_reencode() {
    let matrix = parse_bitmap(ZINT_1234_V1_L4);
    let symbol = HanXinDecoder::new().decode_matrix(&matrix).unwrap();
    let re = HanXinEncoder::new().encode(&symbol).unwrap();
    assert_eq!(matrix_of(&re), &matrix);
}

/// Building "1234" from scratch selects V1 / L4 / mask 2 and matches zint exactly.
#[test]
fn build_1234_matches_zint() {
    let enc = HanXinEncoder::new();
    let symbol = enc.build_numeric("1234", EcLevel::L4).unwrap();
    let meta = match &symbol.meta {
        SymbolMeta::HanXin(m) => m,
        _ => panic!("expected HanXinMeta"),
    };
    assert_eq!(meta.version, Version::new(1).unwrap());
    assert_eq!(meta.mask, Mask::new(2).unwrap());

    let encd = enc.encode(&symbol).unwrap();
    assert_eq!(matrix_of(&encd), &parse_bitmap(ZINT_1234_V1_L4));
}

/// Full identity: build → encode → decode → re-encode is byte-for-byte stable, and
/// the decoded segments/meta match the originals, across modes, versions and levels.
fn assert_roundtrip(segments: Vec<Segment>, level: EcLevel) {
    let enc = HanXinEncoder::new();
    let symbol = enc.build(segments.clone(), level).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = HanXinDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, segments, "segments mismatch");
    assert_eq!(decoded.meta, symbol.meta, "meta mismatch");

    let re = enc.encode(&decoded).unwrap();
    assert_eq!(re, encoding, "re-encode not identical");
}

#[test]
fn roundtrip_numeric() {
    for level in [EcLevel::L1, EcLevel::L2, EcLevel::L3, EcLevel::L4] {
        assert_roundtrip(vec![Segment::numeric(b"0123456789".to_vec())], level);
        assert_roundtrip(vec![Segment::numeric(b"42".to_vec())], level);
        assert_roundtrip(vec![Segment::numeric(b"7".to_vec())], level);
    }
}

#[test]
fn roundtrip_text() {
    // Text 1 (letters/digits) and Text 2 (space/punct) with sub-mode switches.
    assert_roundtrip(
        vec![Segment::alphanumeric(b"Hello World".to_vec())],
        EcLevel::L2,
    );
    assert_roundtrip(
        vec![Segment::alphanumeric(b"abcXYZ123".to_vec())],
        EcLevel::L1,
    );
    assert_roundtrip(
        vec![Segment::alphanumeric(b"a.b:c/d[e]".to_vec())],
        EcLevel::L3,
    );
}

#[test]
fn roundtrip_binary() {
    assert_roundtrip(
        vec![Segment::byte(b"\x00\x01\xfe\xff hi!".to_vec())],
        EcLevel::L1,
    );
    assert_roundtrip(
        vec![Segment::byte("Ünïcödé".as_bytes().to_vec())],
        EcLevel::L2,
    );
}

#[test]
fn roundtrip_multi_segment() {
    assert_roundtrip(
        vec![
            Segment::numeric(b"2024".to_vec()),
            Segment::alphanumeric(b"Item".to_vec()),
            Segment::byte(b"\xff\x00".to_vec()),
        ],
        EcLevel::L2,
    );
}

#[test]
fn roundtrip_larger_versions() {
    // 50 digits (~184 bits) exceeds V1-L1 (168) → forces version 2.
    let n2 = "1234567890".repeat(5);
    let s2 = assert_roundtrip_versioned(vec![Segment::numeric(n2.into_bytes())], EcLevel::L1);
    assert_eq!(s2, Version::new(2).unwrap());
    // 70 digits (~254 bits) exceeds V2-L1 (248) → forces version 3.
    let n3 = "1234567890".repeat(7);
    let s3 = assert_roundtrip_versioned(vec![Segment::numeric(n3.into_bytes())], EcLevel::L1);
    assert_eq!(s3, Version::new(3).unwrap());
}

/// Like [`assert_roundtrip`] but returns the auto-chosen version.
fn assert_roundtrip_versioned(segments: Vec<Segment>, level: EcLevel) -> Version {
    let enc = HanXinEncoder::new();
    let symbol = enc.build(segments.clone(), level).unwrap();
    let version = match &symbol.meta {
        SymbolMeta::HanXin(m) => m.version,
        _ => panic!("expected HanXinMeta"),
    };
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = HanXinDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, segments);
    assert_eq!(decoded.meta, symbol.meta);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
    version
}

#[test]
fn error_correction_repairs_flips() {
    // Flip a handful of data modules; L4's 16 EC codewords (8-error capacity) recover.
    let enc = HanXinEncoder::new();
    let symbol = enc.build_numeric("1234", EcLevel::L4).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let mut matrix = matrix_of(&encoding).clone();
    // Flip three interior data modules well away from the finders.
    for &(x, y) in &[(10usize, 10usize), (12, 11), (11, 13)] {
        matrix.set(x, y, !matrix.get(x, y));
    }
    let decoded = HanXinDecoder::new().decode_matrix(&matrix).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("1234"));
}

#[test]
fn rejects_non_hanxin_symbol() {
    let sym = Symbol::new(
        Symbology::QrCode,
        vec![Segment::numeric(b"1".to_vec())],
        SymbolMeta::HanXin(HanXinMeta::default()),
    );
    assert!(HanXinEncoder::new().encode(&sym).is_err());
}

#[test]
fn modes_helper() {
    let enc = HanXinEncoder::new();
    let symbol = enc
        .build(
            vec![
                Segment::numeric(b"12".to_vec()),
                Segment::byte(b"x".to_vec()),
            ],
            EcLevel::L1,
        )
        .unwrap();
    assert_eq!(symbol.modes(), vec![Mode::Numeric, Mode::Byte]);
}
