//! End-to-end QR round-trip tests: encode → decode → re-encode must be identical,
//! across modes, EC levels and versions, and must survive correctable errors.
#![cfg(all(feature = "decode", feature = "encode", feature = "qr"))]

use anyd::codes::qr::{EcLevel, Mask, QrDecoder, QrEncoder, QrMeta, Version};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
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

/// FNC1 (GS1 / AIM) and Structured Append cannot be represented by `Mode`. A symbol
/// using them passes Reed–Solomon, so the decoder must report the real reason —
/// not retry every format and end on a misleading `ErrorCorrectionFailed`. Bitmap:
/// `zint -b 58 --gs1 -d "[01]12345678901231"` (version 1).
#[test]
fn fnc1_symbol_is_reported_as_unsupported() {
    let flat = "111111100011001111111100000100011101000001101110101110101011101101110101001001011101101110100100001011101100000100100001000001111111101010101111111000000000000000000000000110110001000001100111000011111001110001010101101010110100001010110000000110101100100101110011100100011000000001010101110011111111101111100010000100000100111010010001101110101001110111001101110101100110001110101110100110110000011100000100010011101101111111100111001001000";
    assert_eq!(flat.len(), 21 * 21);
    let mut m = anyd::output::BitMatrix::new(21, 21, 4);
    for (i, b) in flat.bytes().enumerate() {
        m.set(i % 21, i / 21, b == b'1');
    }
    let err = QrDecoder::new().decode(&Encoding::Matrix(m)).unwrap_err();
    assert!(matches!(err, anyd::Error::Unsupported { .. }), "{err:?}");
}

/// A matrix from zint's `--dump` rows (hex, most significant bit first).
fn matrix_from_hex(rows: &[&str]) -> anyd::output::BitMatrix {
    let size = rows.len();
    let mut m = anyd::output::BitMatrix::new(size, size, 4);
    for (y, row) in rows.iter().enumerate() {
        for x in 0..size {
            let nibble = (row.as_bytes()[x / 4] as char).to_digit(16).unwrap();
            m.set(x, y, nibble & (8 >> (x % 4)) != 0);
        }
    }
    m
}

/// Structured Append (mode `0011`), like FNC1, is reported as unsupported. Bitmap:
/// `zint -b 58 --vers=1 --structapp=2,3 -d "A"`.
#[test]
fn structured_append_symbol_is_reported_as_unsupported() {
    let m = matrix_from_hex(&[
        "FE4BF8", "829208", "BABAE8", "BAEAE8", "BAB2E8", "82F208", "FEABF8", "003000", "27F5F0",
        "392608", "7BE078", "789C80", "670C78", "00FB20", "FEFBB8", "828B48", "BA3BF8", "BA7410",
        "BAD478", "824A98", "FE6078",
    ]);
    let err = QrDecoder::new().decode(&Encoding::Matrix(m)).unwrap_err();
    assert!(matches!(err, anyd::Error::Unsupported { .. }), "{err:?}");
}

/// An independent full-symbol vector with everything a larger version adds: version
/// information, alignment patterns and five interleaved RS blocks of two sizes.
/// `zint -b 58 --vers=7 --secure=4 --mask=5 -d "ANYD-7H 0123456789 lossless"` must
/// decode to that text and re-encode to zint's exact 45×45 modules.
#[test]
fn version_7_h_matches_zint() {
    let m = matrix_from_hex(&[
        "FEC805420BF8",
        "82254D721208",
        "BAC119E412E8",
        "BA346FD15AE8",
        "BAE88FDCFAE8",
        "825A38EB8208",
        "FEAAAAAAABF8",
        "00DF88831000",
        "06750FDE1AA8",
        "BC191923EBD8",
        "33C7A92E5D00",
        "6D93450C0E50",
        "7AEA1CEC5878",
        "9C4D83C4B0C0",
        "7222189BA4F8",
        "114C5F62B180",
        "2744FDFE1578",
        "18581E420B30",
        "96C0BA63B1F0",
        "C0B982357FA0",
        "7FA78FD96FD0",
        "D88D889D48B8",
        "EAEC6AFA4AE0",
        "B8AA78C318A0",
        "DF979F81BFA8",
        "D8B44D690AD0",
        "D6097D521680",
        "90219B72ABD8",
        "83762B03C120",
        "F0EDC09CCDA0",
        "DA2D343A5E20",
        "80CF030125A8",
        "B706DA4C1F78",
        "44F50B279158",
        "0B7A2E2637A0",
        "78D5622069B0",
        "9B526FE55FF8",
        "00C088EB78E0",
        "FE449AA0CA80",
        "82C3C8DD28A0",
        "BA66DFB88FC8",
        "BA377FC89340",
        "BA682F9BB3D8",
        "8238034A7948",
        "FE207869E120",
    ]);
    let decoded = QrDecoder::new().decode_matrix(&m).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("ANYD-7H 0123456789 lossless")
    );
    let SymbolMeta::Qr(meta) = &decoded.meta else {
        panic!("expected QrMeta");
    };
    assert_eq!(
        (meta.version.number(), meta.ec_level, meta.mask.index()),
        (7, EcLevel::H, 5)
    );
    assert_eq!(
        QrEncoder::new().encode(&decoded).unwrap(),
        Encoding::Matrix(m)
    );
}

/// 8-bit byte capacity of every version at L, M, Q, H — the "Data capacity" column
/// of ISO/IEC 18004 Table 7, confirmed against zint 2.16 (`--vers`, `--secure`) for
/// all 160 cells.
#[rustfmt::skip]
const BYTE_CAPACITY: [[usize; 4]; 40] = [
    [17, 14, 11, 7], [32, 26, 20, 14], [53, 42, 32, 24], [78, 62, 46, 34],
    [106, 84, 60, 44], [134, 106, 74, 58], [154, 122, 86, 64], [192, 152, 108, 84],
    [230, 180, 130, 98], [271, 213, 151, 119], [321, 251, 177, 137], [367, 287, 203, 155],
    [425, 331, 241, 177], [458, 362, 258, 194], [520, 412, 292, 220], [586, 450, 322, 250],
    [644, 504, 364, 280], [718, 560, 394, 310], [792, 624, 442, 338], [858, 666, 482, 382],
    [929, 711, 509, 403], [1003, 779, 565, 439], [1091, 857, 611, 461], [1171, 911, 661, 511],
    [1273, 997, 715, 535], [1367, 1059, 751, 593], [1465, 1125, 805, 625],
    [1528, 1190, 868, 658], [1628, 1264, 908, 698], [1732, 1370, 982, 742],
    [1840, 1452, 1030, 790], [1952, 1538, 1112, 842], [2068, 1628, 1168, 898],
    [2188, 1722, 1228, 958], [2303, 1809, 1283, 983], [2431, 1911, 1351, 1051],
    [2563, 1989, 1423, 1093], [2699, 2099, 1499, 1139], [2809, 2213, 1579, 1219],
    [2953, 2331, 1663, 1273],
];
const LEVELS: [EcLevel; 4] = [EcLevel::L, EcLevel::M, EcLevel::Q, EcLevel::H];

/// The longest numeric, alphanumeric, byte and Kanji segment of a version/level,
/// derived from the byte capacity: byte mode spends 4 + 8 (versions 1–9) or 4 + 16
/// header bits, which pins the data codeword count.
fn capacities(version: u8, level_index: usize) -> [usize; 4] {
    let band = match version {
        1..=9 => 0,
        10..=26 => 1,
        _ => 2,
    };
    let byte = BYTE_CAPACITY[version as usize - 1][level_index];
    let bits = 8 * (byte + [2, 3, 3][band]) - 4;
    let numeric = bits - [10, 12, 14][band];
    let alnum = bits - [9, 11, 13][band];
    let kanji = bits - [8, 10, 12][band];
    [
        numeric / 10 * 3 + [0, 0, 0, 0, 1, 1, 1, 2, 2, 2][numeric % 10],
        alnum / 11 * 2 + usize::from(alnum % 11 >= 6),
        byte,
        kanji / 13,
    ]
}

fn segment_of(mode: usize, len: usize) -> Segment {
    match mode {
        0 => Segment::numeric(vec![b'8'; len]),
        1 => Segment::alphanumeric(vec![b'%'; len]),
        2 => Segment::byte(vec![0xB7; len]),
        _ => Segment::kanji([0xE4, 0xAA].repeat(len)),
    }
}

fn pinned(segments: Vec<Segment>, version: u8, level: EcLevel) -> Symbol {
    let meta = QrMeta {
        version: Version::new(version).unwrap(),
        ec_level: level,
        mask: Mask::new(version % 8).unwrap(),
    };
    Symbol::new(Symbology::QrCode, segments, SymbolMeta::Qr(meta))
}

/// The capacity derivation reproduces the ISO/IEC 18004 Table 7 rows quoted
/// everywhere (numeric, alphanumeric, byte, Kanji).
#[test]
fn capacity_derivation_matches_iso_table_7() {
    assert_eq!(capacities(1, 0), [41, 25, 17, 10]);
    assert_eq!(capacities(1, 3), [17, 10, 7, 4]);
    assert_eq!(capacities(10, 1), [513, 311, 213, 131]);
    assert_eq!(capacities(40, 0), [7089, 4296, 2953, 1817]);
    assert_eq!(capacities(40, 3), [3057, 1852, 1273, 784]);
}

/// At every version and level, in every mode: the Table 7 maximum encodes at that
/// pinned version and the builder picks exactly that version; one character more
/// is a capacity error there (never a panic or a silently shortened symbol) and
/// sends the builder to the next version, or fails past version 40.
#[test]
fn capacity_boundaries_at_every_version_and_level() {
    let enc = QrEncoder::new();
    let mut scratch = vec![0u8; QrEncoder::MAX_BUFFER_LEN];
    let mut storage = vec![0u8; QrEncoder::MAX_BUFFER_LEN];
    // The mask is pinned so the 640 cells do not each pay for a mask search.
    let mask = Mask::new(0);
    let mut chosen = |segment: &Segment, level, version: Option<Version>| {
        enc.encode_into(
            &[segment.view()],
            level,
            version,
            mask,
            &mut scratch,
            &mut storage,
        )
        .map(|(_, meta)| meta.version.number())
    };
    for v in 1..=40u8 {
        let version = Version::new(v);
        for (li, &level) in LEVELS.iter().enumerate() {
            for (mode, &max) in capacities(v, li).iter().enumerate() {
                // All four modes in the small versions; above them one per cell, in
                // rotation, keeps the test quick (every mode still meets every
                // count-field width and every level).
                if v > 10 && mode != (v as usize + li) % 4 {
                    continue;
                }
                let what = format!("v{v} {level:?} mode {mode}");
                let full = segment_of(mode, max);
                assert_eq!(chosen(&full, level, None), Ok(v), "{what}");

                let over = segment_of(mode, max + 1);
                assert!(
                    matches!(
                        chosen(&over, level, version),
                        Err(anyd::Error::Capacity { .. })
                    ),
                    "{what}"
                );
                match chosen(&over, level, None) {
                    Ok(next) => assert_eq!(next, v + 1, "{what}"),
                    Err(e) => {
                        assert_eq!(v, 40, "{what}");
                        assert!(matches!(e, anyd::Error::Capacity { .. }), "{what}");
                    }
                }
            }
        }
    }
}

/// A full symbol decodes back to itself: no terminator, no padding, and (in the
/// versions that have them) remainder bits after the last codeword.
#[test]
fn full_symbols_roundtrip() {
    let enc = QrEncoder::new();
    for (version, li) in [
        (1, 0),
        (2, 3),
        (5, 2),
        (7, 1),
        (9, 0),
        (10, 3),
        (14, 2),
        (26, 1),
        (27, 0),
        (40, 3),
    ] {
        for (mode, &max) in capacities(version, li).iter().enumerate() {
            let symbol = pinned(vec![segment_of(mode, max)], version, LEVELS[li]);
            let encoding = enc.encode(&symbol).unwrap();
            let decoded = QrDecoder::new().decode(&encoding).unwrap();
            assert_eq!(decoded, symbol, "v{version} mode {mode}");
        }
    }
    // build_text reaches the top of the range and stops there.
    let digits = "7".repeat(7089);
    let symbol = enc.build_text(&digits, EcLevel::L).unwrap();
    assert_eq!(symbol.segments, [Segment::numeric(digits.clone())]);
    assert!(matches!(
        enc.build_text(&(digits + "7"), EcLevel::L),
        Err(anyd::Error::Capacity { .. })
    ));
}

/// The count field is 8/16 bits for bytes and grows with the version band for the
/// other modes; a segment at each band's edge keeps its length.
#[test]
fn character_count_widths_change_at_versions_10_and_27() {
    for (version, level) in [
        (9, EcLevel::L),
        (10, EcLevel::L),
        (26, EcLevel::M),
        (27, EcLevel::M),
    ] {
        let symbol = pinned(
            vec![
                Segment::numeric(vec![b'1'; 130]),
                Segment::alphanumeric(vec![b'Z'; 33]),
                Segment::byte(vec![b'q'; 21]),
                Segment::kanji([0x88, 0x9F].repeat(9)),
            ],
            version,
            level,
        );
        let encoding = QrEncoder::new().encode(&symbol).unwrap();
        assert_eq!(QrDecoder::new().decode(&encoding).unwrap(), symbol);
    }
}

/// ECI designators use 1, 2 or 3 bytes; each form's limits round-trip, and numbers
/// past 999999 are refused.
#[test]
fn eci_designators_roundtrip_in_every_length() {
    for assignment in [0, 3, 26, 127, 128, 899, 16_383, 16_384, 999_999] {
        assert_lossless(
            vec![
                Segment::eci(assignment),
                Segment::byte("ECI".as_bytes().to_vec()),
                Segment::eci(20),
                Segment::kanji(vec![0x93, 0x5F]),
            ],
            EcLevel::M,
        );
    }
    let enc = QrEncoder::new();
    assert!(
        enc.build(vec![Segment::eci(1_000_000)], EcLevel::L)
            .is_err()
    );
    assert!(matches!(
        enc.encode(&pinned(vec![Segment::eci(u32::MAX)], 2, EcLevel::L)),
        Err(anyd::Error::InvalidData { .. })
    ));
}

/// Zero-length segments are content (their mode indicator is not the terminator),
/// and a symbol with no segments at all is valid too.
#[test]
fn empty_segments_roundtrip() {
    for segments in [
        vec![],
        vec![Segment::numeric(Vec::new())],
        vec![Segment::alphanumeric(Vec::new())],
        vec![Segment::byte(Vec::new())],
        vec![Segment::kanji(Vec::new())],
        vec![
            Segment::numeric(Vec::new()),
            Segment::byte(b"x".to_vec()),
            Segment::kanji(Vec::new()),
            Segment::alphanumeric(b"Y".to_vec()),
        ],
    ] {
        assert_lossless(segments, EcLevel::H);
    }
    let enc = QrEncoder::new();
    let symbol = enc.build_text("", EcLevel::Q).unwrap();
    let decoded = QrDecoder::new()
        .decode(&enc.encode(&symbol).unwrap())
        .unwrap();
    assert_eq!(decoded, symbol);
}

/// Every character of the alphanumeric set, in both positions of a pair and as
/// the odd one out.
#[test]
fn alphanumeric_covers_its_whole_alphabet() {
    let alphabet = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
    assert_eq!(alphabet.len(), 45);
    let mut twice = alphabet.to_vec();
    twice.extend(alphabet.iter().rev());
    twice.push(b':');
    assert_lossless(vec![Segment::alphanumeric(twice)], EcLevel::L);
}

#[test]
fn encoder_rejects_what_it_cannot_represent() {
    let enc = QrEncoder::new();
    for segments in [
        vec![Segment::numeric(b"12a".to_vec())],
        vec![Segment::alphanumeric(b"abc".to_vec())],
        vec![Segment::alphanumeric(b"A,B".to_vec())],
        vec![Segment::kanji(vec![0x93])],
        vec![Segment::kanji(vec![0x41, 0x42])],
        vec![Segment::kanji(vec![0xEB, 0xC0])],
    ] {
        // With a pinned version the real reason is reported ...
        let symbol = pinned(segments.clone(), 3, EcLevel::L);
        assert!(
            matches!(enc.encode(&symbol), Err(anyd::Error::InvalidData { .. })),
            "{segments:?}"
        );
        // ... and no automatic version accepts it either.
        assert!(enc.build(segments, EcLevel::L).is_err());
    }
    // Wrong symbology, and the right one without QR metadata.
    let mut symbol = pinned(vec![Segment::byte(b"x".to_vec())], 1, EcLevel::L);
    symbol.symbology = Symbology::MicroQrCode;
    assert!(matches!(
        enc.encode(&symbol),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    let symbol = Symbol::new(Symbology::QrCode, Vec::new(), SymbolMeta::Generic);
    assert!(matches!(
        enc.encode(&symbol),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    // A buffer too small for the version is an error, not an out-of-bounds write.
    let views = [anyd::segment::SegmentView::byte(b"x")];
    let need = QrEncoder::buffer_len(Version::new(1).unwrap());
    // (The codeword scratch only has to hold version 1's 26 codewords.)
    let (mut short, mut ok) = (vec![0u8; 25], vec![0u8; need]);
    assert!(matches!(
        enc.encode_into(&views, EcLevel::L, None, None, &mut short, &mut ok),
        Err(anyd::Error::Capacity { .. })
    ));
    let (mut short, mut ok) = (vec![0u8; need - 1], vec![0u8; need]);
    assert!(matches!(
        enc.encode_into(&views, EcLevel::L, None, None, &mut ok, &mut short),
        Err(anyd::Error::Capacity { .. })
    ));
}

#[test]
fn decoder_rejects_non_qr_inputs() {
    let dec = QrDecoder::new();
    let linear = Encoding::Linear(anyd::output::LinearPattern::new());
    assert!(matches!(
        dec.decode(&linear),
        Err(anyd::Error::Unsupported { .. })
    ));
    for (w, h) in [(0, 0), (20, 20), (21, 25), (23, 23), (181, 181)] {
        let blank = anyd::output::BitMatrix::new(w, h, 4);
        assert!(dec.decode_matrix(&blank).is_err(), "{w}x{h}");
    }
    assert!(
        dec.decode_matrix(&anyd::output::BitMatrix::new(21, 21, 4))
            .is_err()
    );
}

#[test]
fn parameter_types_validate_their_ranges() {
    assert!(Version::new(0).is_none() && Version::new(41).is_none());
    for v in 1..=40u8 {
        let version = Version::new(v).unwrap();
        assert_eq!(version.number(), v);
        assert_eq!(version.size(), 17 + 4 * v as usize);
        assert!(QrEncoder::buffer_len(version) <= QrEncoder::MAX_BUFFER_LEN);
    }
    assert!(Mask::new(8).is_none());
    assert_eq!(Mask::new(7).unwrap().index(), 7);
    // Format-information level bits: L = 01, M = 00, Q = 11, H = 10.
    for (level, bits) in LEVELS.into_iter().zip([0b01, 0b00, 0b11, 0b10]) {
        assert_eq!(level.format_bits(), bits);
        assert_eq!(EcLevel::from_format_bits(bits), level);
    }
    assert!(EcLevel::L < EcLevel::M && EcLevel::M < EcLevel::Q && EcLevel::Q < EcLevel::H);
}

/// Format information is BCH(15,5): three damaged modules in each copy are still
/// read, at every level and mask; and with both copies wiped out the decoder
/// recovers the format by trying the 32 combinations against Reed–Solomon.
///
/// Version 2 is one RS block at every level, so an M, Q or H block is also a valid
/// L codeword: the search used to try L first and report the symbol as level L,
/// with the tail of the data and the first EC codewords taken for padding.
#[test]
fn format_information_survives_damage() {
    let enc = QrEncoder::new();
    for (li, &level) in LEVELS.iter().enumerate() {
        for mask in 0..8u8 {
            let mut symbol = pinned(vec![Segment::byte(b"format info".to_vec())], 2, level);
            let SymbolMeta::Qr(meta) = &mut symbol.meta else {
                unreachable!()
            };
            meta.mask = Mask::new(mask).unwrap();
            let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
                panic!("expected a matrix");
            };
            // Three modules of each copy: column 8 near the top-left finder, and
            // row 8 under the top-right one.
            let mut m = clean.clone();
            for k in 0..3 {
                let (a, b) = ((8, (mask as usize + li + k) % 6), (24 - k, 8));
                m.set(a.0, a.1, !clean.get(a.0, a.1));
                m.set(b.0, b.1, !clean.get(b.0, b.1));
            }
            assert_eq!(QrDecoder::new().decode_matrix(&m).as_ref(), Ok(&symbol));

            // Both copies blanked entirely (sparing the timing modules at 6).
            let mut m = clean.clone();
            for i in (0..9).filter(|&i| i != 6) {
                m.set(8, i, false);
                m.set(i, 8, false);
                m.set(24 - i.min(7), 8, false);
                m.set(8, 24 - i.min(6), false);
            }
            assert_eq!(QrDecoder::new().decode_matrix(&m).as_ref(), Ok(&symbol));
        }
    }
}
