//! Han Xin Code (ISO/IEC 20830) integration tests.
//!
//! The headline test is an **independent reference vector**: the exact 23×23 module
//! bitmap that zint (`libzint`) emits for the numeric input "1234" at version 1, EC
//! level 4. It is reproduced verbatim from zint's own test suite
//! (`backend/tests/test_hanxin.c`, which cross-checks against ZXing-C++), decoded
//! here, and re-encoded to the identical bitmap. This pins the finder patterns,
//! numeric mode, GF(2^8)/`0x163` Reed–Solomon, picket-fence interleave, mask 2 and
//! the function information all at once against a third-party implementation.
#![cfg(all(feature = "decode", feature = "encode", feature = "hanxin"))]

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

// --- Automatic mode selection in build_text ---

/// Build from text, assert the chosen segmentation, and require a full round trip.
fn assert_text_segments(text: &str, want: &[Segment]) {
    let enc = HanXinEncoder::new();
    let sym = enc.build_text(text, EcLevel::L1).unwrap();
    assert_eq!(sym.segments, want, "unexpected segmentation for {text:?}");
    let encoding = enc.encode(&sym).unwrap();
    let decoded = HanXinDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, sym.segments);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn build_text_digits_pick_numeric() {
    assert_text_segments("1234567890", &[Segment::numeric(b"1234567890".to_vec())]);
}

#[test]
fn build_text_ascii_picks_text_mode() {
    // Han Xin's text mode covers lowercase too (sub-modes 1 and 2).
    assert_text_segments(
        "Hello World",
        &[Segment::alphanumeric(b"Hello World".to_vec())],
    );
}

#[test]
fn build_text_non_ascii_falls_back_to_binary() {
    let text = "café \u{1F600}";
    let enc = HanXinEncoder::new();
    let sym = enc.build_text(text, EcLevel::L1).unwrap();
    let flat: Vec<u8> = sym.segments.iter().flat_map(|s| s.data.clone()).collect();
    assert_eq!(flat, text.as_bytes().to_vec());
    let encoding = enc.encode(&sym).unwrap();
    let decoded = HanXinDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, sym.segments);
}

// --- More zint reference vectors: versions 2 and 3, text and binary modes ---

/// `zint -b 116 --vers=2 --secure=2 --mask=1 -d "Han Xin: text & 42"` (25×25): text
/// mode with both sub-modes and the switches between them.
const ZINT_TEXT_V2_L2_M1: &str = "\
1111111010111000001111111
1000000001101010000000001
1011111011100101001111101
1010000000000011000000101
1010111000001001101110101
1010111010011100001110101
1010111001100101101110101
0000000010010010100000000
0001011000111001011000000
1011010110100110110010100
0011000110010100101110100
1000100001011001000010101
1110100110011001101010011
0101011010100101010100111
1000000110111010101011100
1100110010110101010101000
0000001101111100001101000
0000000010100101100000000
1111111010101001001110101
0000001000010011101110101
1111101011101100001110101
0000101001010101000000101
1110101001110100101111101
1110101000011110000000001
1110101001101010101111111";

/// `zint -b 116 --vers=3 --secure=3 --mask=3 --binary --esc -d
/// "\x00\xFF\x80anyd\x7F"` (27×27): binary mode.
const ZINT_BINARY_V3_L3_M3: &str = "\
111111100010111011101111111
100000000001110000100000001
101111101001000100101111101
101000000111011110100000101
101011101100000001001110101
101011101001100111001110101
101011101010111100101110101
000000000001110001000000000
000101111001110001010000000
011110011011011101110000001
100111010011010010001010001
110001011101100111100101010
100001011011100011100110101
011100010101010001101010100
011010100111011001110001100
011100110000010000011110001
100110110111000001111110000
110011100001011101010010110
000000010011011011111101000
000000000011111110000000000
111111101100111011101110101
000000100010100100101110101
111110100110100101101110101
000010101011011111000000101
111010101001011011101111101
111010101001111110000000001
111010101011100100001111111";

fn meta(version: u8, level: EcLevel, mask: u8) -> SymbolMeta {
    SymbolMeta::HanXin(HanXinMeta {
        version: Version::new(version).unwrap(),
        ec_level: level,
        mask: Mask::new(mask).unwrap(),
    })
}

/// A zint bitmap must decode to `want`, and `want` must encode to zint's bitmap.
fn assert_matches_zint(bitmap: &str, want: &Symbol) {
    let matrix = parse_bitmap(bitmap);
    let decoded = HanXinDecoder::new().decode_matrix(&matrix).unwrap();
    assert_eq!(&decoded, want);
    let encoding = HanXinEncoder::new().encode(want).unwrap();
    assert_eq!(matrix_of(&encoding), &matrix);
}

#[test]
fn text_mode_v2_matches_zint() {
    let want = Symbol::new(
        Symbology::HanXin,
        vec![Segment::alphanumeric(b"Han Xin: text & 42".to_vec())],
        meta(2, EcLevel::L2, 1),
    );
    assert_matches_zint(ZINT_TEXT_V2_L2_M1, &want);
}

#[test]
fn binary_mode_v3_matches_zint() {
    let want = Symbol::new(
        Symbology::HanXin,
        vec![Segment::byte(b"\x00\xFF\x80anyd\x7F".to_vec())],
        meta(3, EcLevel::L3, 3),
    );
    assert_matches_zint(ZINT_BINARY_V3_L3_M3, &want);
}

// --- Capacity boundaries ---

/// Longest numeric / binary payload per (version, EC level L1..L4), as accepted by
/// zint 2.16 at that pinned size (`--vers`, `--secure`); these follow from the data
/// codeword counts 21/17/13/9, 31/25/19/15 and 42/34/26/20.
const NUMERIC_CAPACITY: [[usize; 4]; 3] = [[45, 36, 27, 15], [69, 54, 39, 30], [96, 75, 57, 42]];
const BINARY_CAPACITY: [[usize; 4]; 3] = [[18, 14, 10, 6], [28, 22, 16, 12], [39, 31, 23, 17]];
const LEVELS: [EcLevel; 4] = [EcLevel::L1, EcLevel::L2, EcLevel::L3, EcLevel::L4];
/// Builds a segment of `n` characters.
type MakeSegment = fn(usize) -> Segment;

/// At every version and level the documented maximum fits and round-trips, one
/// character more is a capacity error (never a panic or a truncated symbol), and
/// the builder moves up exactly at that boundary.
#[test]
fn capacity_boundaries_at_every_version_and_level() {
    let enc = HanXinEncoder::new();
    let version_of = |s: &Symbol| match &s.meta {
        SymbolMeta::HanXin(m) => m.version.number(),
        _ => panic!("expected HanXinMeta"),
    };
    for v in 1..=3u8 {
        for (li, &level) in LEVELS.iter().enumerate() {
            let cases: [(usize, MakeSegment); 2] = [
                (NUMERIC_CAPACITY[v as usize - 1][li], |n| {
                    Segment::numeric(vec![b'7'; n])
                }),
                (BINARY_CAPACITY[v as usize - 1][li], |n| {
                    Segment::byte(vec![0xA5; n])
                }),
            ];
            for (max, make) in cases {
                let full = Symbol::new(Symbology::HanXin, vec![make(max)], meta(v, level, 2));
                let encoding = enc.encode(&full).unwrap();
                assert_eq!(HanXinDecoder::new().decode(&encoding).unwrap(), full);
                assert_eq!(version_of(&enc.build(vec![make(max)], level).unwrap()), v);

                let over = Symbol::new(Symbology::HanXin, vec![make(max + 1)], meta(v, level, 2));
                assert!(
                    matches!(enc.encode(&over), Err(anyd::Error::Capacity { .. })),
                    "v{v} {level:?}"
                );
                match enc.build(vec![make(max + 1)], level) {
                    Ok(sym) => assert_eq!(version_of(&sym), v + 1, "v{v} {level:?}"),
                    Err(e) => {
                        assert_eq!(v, 3, "v{v} {level:?}");
                        assert!(matches!(e, anyd::Error::Capacity { .. }));
                    }
                }
            }
        }
    }
}

// --- Segment edge cases ---

/// Empty segments of every mode are content, not terminators: they survive the
/// round trip, alone and between other segments.
#[test]
fn empty_segments_roundtrip() {
    for segments in [
        vec![],
        vec![Segment::numeric(Vec::new())],
        vec![Segment::alphanumeric(Vec::new())],
        vec![Segment::byte(Vec::new())],
        vec![
            Segment::numeric(Vec::new()),
            Segment::alphanumeric(b"a".to_vec()),
            Segment::byte(Vec::new()),
            Segment::alphanumeric(Vec::new()),
            Segment::numeric(b"7".to_vec()),
        ],
    ] {
        assert_roundtrip(segments, EcLevel::L1);
    }
    let enc = HanXinEncoder::new();
    let sym = enc.build_text("", EcLevel::L3).unwrap();
    let decoded = HanXinDecoder::new()
        .decode(&enc.encode(&sym).unwrap())
        .unwrap();
    assert_eq!(decoded, sym);
}

/// Text mode holds every ASCII byte except the four controls 28..=31 (sub-mode 2
/// has 62 values: 0..=27, punctuation, DEL); each one round-trips, interleaved
/// with the other sub-mode so every switch is exercised.
#[test]
fn text_mode_covers_its_whole_alphabet() {
    let alphabet: Vec<u8> = (0u8..=127).filter(|b| !(28..=31).contains(b)).collect();
    assert_eq!(alphabet.len(), 124);
    for chunk in alphabet.chunks(12) {
        let mut interleaved = Vec::new();
        for (i, &b) in chunk.iter().enumerate() {
            interleaved.push(b);
            interleaved.push(b"aZ5{ "[i % 5]);
        }
        assert_roundtrip(vec![Segment::alphanumeric(interleaved)], EcLevel::L1);
    }
    let enc = HanXinEncoder::new();
    for b in (28u8..=31).chain(128..=255) {
        let err = enc.build(vec![Segment::alphanumeric(vec![b'A', b])], EcLevel::L1);
        assert!(
            matches!(err, Err(anyd::Error::InvalidData { .. })),
            "byte {b:#04x}"
        );
    }
}

/// Numeric groups of 1, 2 and 3 trailing digits, with leading zeros.
#[test]
fn numeric_group_remainders_roundtrip() {
    for digits in [
        "0",
        "00",
        "000",
        "0000",
        "00000",
        "9",
        "99",
        "999",
        "1000",
        "007",
        "0123456789",
    ] {
        assert_roundtrip(
            vec![Segment::numeric(digits.as_bytes().to_vec())],
            EcLevel::L2,
        );
    }
}

// --- build_text ---

/// The segmenter does not see the 6-bit Text sub-mode switches, so on text that
/// alternates letters and punctuation its "cheaper" text segment used to come out
/// longer than plain binary — up to a capacity error on data that fits. The
/// result must never need a larger version than a single binary segment.
#[test]
fn build_text_never_worse_than_binary() {
    let enc = HanXinEncoder::new();
    let version_of = |s: &Symbol| match &s.meta {
        SymbolMeta::HanXin(m) => m.version.number(),
        _ => panic!("expected HanXinMeta"),
    };
    // Fits version 3 at L4 as binary; the text split overflowed it.
    let sym = enc.build_text("q8362s?t&e?H&e?j", EcLevel::L4).unwrap();
    assert_eq!(version_of(&sym), 3);
    // Version 1 as binary; the text split needed version 2.
    let sym = enc.build_text("d% E+%$T.Q", EcLevel::L3).unwrap();
    assert_eq!(version_of(&sym), 1);

    let mut rng = anyd::transform::Rng::new(0x4858_7E87);
    for _ in 0..600 {
        let len = 1 + (rng.next_u64() % 40) as usize;
        let text: String = (0..len)
            .map(|_| b"aB3?&e 9.Zq%"[(rng.next_u64() % 12) as usize] as char)
            .collect();
        let level = LEVELS[(rng.next_u64() % 4) as usize];
        let binary = enc.build(vec![Segment::byte(text.as_bytes().to_vec())], level);
        match (enc.build_text(&text, level), binary) {
            (Ok(auto), Ok(binary)) => {
                assert!(version_of(&auto) <= version_of(&binary), "{text:?}");
                let decoded = HanXinDecoder::new()
                    .decode(&enc.encode(&auto).unwrap())
                    .unwrap();
                assert_eq!(decoded.text().as_deref(), Some(text.as_str()));
            }
            (Ok(_), Err(_)) => {} // only the denser split fits
            (Err(e), binary) => {
                assert!(binary.is_err(), "{text:?} fits as binary, not as text: {e}");
            }
        }
    }
}

// --- Error paths and accessors ---

#[test]
fn encoder_rejects_what_it_cannot_represent() {
    let enc = HanXinEncoder::new();
    for segments in [
        vec![Segment::kanji(vec![0x93, 0x5F])],
        vec![Segment::eci(26), Segment::byte(b"x".to_vec())],
    ] {
        assert!(matches!(
            enc.build(segments, EcLevel::L1),
            Err(anyd::Error::Unsupported { .. })
        ));
    }
    assert!(matches!(
        enc.build(vec![Segment::numeric(b"12a".to_vec())], EcLevel::L1),
        Err(anyd::Error::InvalidData { .. })
    ));
    assert!(enc.build_numeric("12 3", EcLevel::L1).is_err());
    // The right symbology with the wrong metadata.
    let sym = Symbol::new(
        Symbology::HanXin,
        vec![Segment::numeric(b"1".to_vec())],
        SymbolMeta::Generic,
    );
    assert!(matches!(
        enc.encode(&sym),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    // More than versions 1–3 hold.
    assert!(matches!(
        enc.build(vec![Segment::byte(vec![0; 40])], EcLevel::L1),
        Err(anyd::Error::Capacity { .. })
    ));
}

#[test]
fn decoder_rejects_non_han_xin_inputs() {
    let dec = HanXinDecoder::new();
    let linear = Encoding::Linear(anyd::output::LinearPattern::new());
    assert!(matches!(
        dec.decode(&linear),
        Err(anyd::Error::Unsupported { .. })
    ));
    // Not square, even side, below version 1, and version 4 (unsupported).
    for (w, h) in [(23, 25), (24, 24), (21, 21), (29, 29), (0, 0)] {
        assert!(
            dec.decode_matrix(&BitMatrix::new(w, h, 3)).is_err(),
            "{w}x{h}"
        );
    }
    // A blank grid of a valid size has no readable function information.
    assert!(dec.decode_matrix(&BitMatrix::new(23, 23, 3)).is_err());
}

#[test]
fn parameter_types_validate_their_ranges() {
    assert!(Version::new(0).is_none() && Version::new(4).is_none());
    for v in 1..=3u8 {
        let version = Version::new(v).unwrap();
        assert_eq!(version.number(), v);
        assert_eq!(version.size(), 21 + 2 * v as usize);
    }
    assert!(Mask::new(4).is_none());
    assert_eq!(Mask::new(3).unwrap().index(), 3);
    for (i, level) in LEVELS.into_iter().enumerate() {
        assert_eq!(level.field_value() as usize, i);
        assert_eq!(EcLevel::from_field_value(i as u8), Some(level));
    }
    let default = HanXinMeta::default();
    assert_eq!(
        (
            default.version.number(),
            default.ec_level,
            default.mask.index()
        ),
        (1, EcLevel::L1, 0)
    );
}

/// Every level corrects at least `ec / 2` damaged modules (each module is at most
/// one codeword error) anywhere in the data region, in every version.
#[test]
fn corrects_half_the_ec_codewords_everywhere() {
    const EC: [[usize; 4]; 3] = [[4, 8, 12, 16], [6, 12, 18, 22], [8, 16, 24, 30]];
    let enc = HanXinEncoder::new();
    let mut rng = anyd::transform::Rng::new(0x00EC_C0DE);
    for v in 1..=3u8 {
        for (li, &level) in LEVELS.iter().enumerate() {
            let symbol = Symbol::new(
                Symbology::HanXin,
                vec![Segment::alphanumeric(b"Han".to_vec())],
                meta(v, level, (v + li as u8) % 4),
            );
            let encoding = enc.encode(&symbol).unwrap();
            let clean = matrix_of(&encoding);
            let size = clean.width();
            for _ in 0..25 {
                let mut m = clean.clone();
                for _ in 0..EC[v as usize - 1][li] / 2 {
                    // The interior is all data: finders and function information
                    // stay within 9 modules of the corners.
                    let x = 9 + (rng.next_u64() as usize) % (size - 18);
                    let y = 9 + (rng.next_u64() as usize) % (size - 18);
                    m.set(x, y, !clean.get(x, y));
                }
                assert_eq!(
                    HanXinDecoder::new().decode_matrix(&m).as_ref(),
                    Ok(&symbol),
                    "v{v} {level:?}"
                );
            }
        }
    }
}
