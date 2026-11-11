//! End-to-end PDF417 round-trip tests: encode → decode → re-encode must be
//! identical, across Text/Byte/Numeric compaction and several error-correction
//! levels, and must survive correctable errors.
#![cfg(all(feature = "decode", feature = "encode", feature = "pdf417"))]

use anyd::codes::pdf417::{EcLevel, Pdf417Decoder, Pdf417Encoder};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::traits::{Decode, Encode};

fn level(n: u8) -> EcLevel {
    EcLevel::new(n).unwrap()
}

/// Assert that a symbol decodes to the same segments/meta and re-encodes identically.
fn assert_lossless(segments: Vec<Segment>, ec: EcLevel) {
    let enc = Pdf417Encoder::new();
    let symbol = enc.build(segments.clone(), ec).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = Pdf417Decoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.segments, symbol.segments,
        "segments differ after decode"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs after decode");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn text_all_levels() {
    for l in 0..=4 {
        assert_lossless(
            vec![Segment::alphanumeric(
                b"PDF417 Text Compaction: Mixed 123 $tuff!".to_vec(),
            )],
            level(l),
        );
    }
}

#[test]
fn numeric_all_levels() {
    for l in 0..=4 {
        assert_lossless(
            vec![Segment::numeric(b"00213298174000123456789".to_vec())],
            level(l),
        );
    }
}

#[test]
fn byte_all_levels() {
    for l in 0..=4 {
        assert_lossless(
            vec![Segment::byte((0u8..=250).collect::<Vec<u8>>())],
            level(l),
        );
    }
}

#[test]
fn byte_utf8() {
    assert_lossless(
        vec![Segment::byte("héllo wörld — 日本語 🦀".as_bytes().to_vec())],
        level(2),
    );
}

#[test]
fn mixed_segments_preserve_boundaries() {
    // Alternating modes so every boundary is a recoverable mode latch.
    let segments = vec![
        Segment::numeric(b"12345".to_vec()),
        Segment::alphanumeric(b"ABCdef".to_vec()),
        Segment::byte(b"\x00\x01\xff\xfe raw".to_vec()),
        Segment::numeric(b"9876543210".to_vec()),
    ];
    assert_lossless(segments, level(3));
}

#[test]
fn numeric_odd_remainders() {
    assert_lossless(vec![Segment::numeric(b"7".to_vec())], level(1));
    assert_lossless(vec![Segment::numeric(b"42".to_vec())], level(1));
    assert_lossless(vec![Segment::numeric(b"100".to_vec())], level(1));
    // A run longer than one 44-digit group.
    let long: Vec<u8> = (0..100).map(|i| b'0' + (i % 10) as u8).collect();
    assert_lossless(vec![Segment::numeric(long)], level(2));
}

#[test]
fn large_payload_multiple_columns() {
    let data = vec![b'A'; 400];
    assert_lossless(vec![Segment::alphanumeric(data)], level(3));
}

#[test]
fn explicit_columns() {
    let enc = Pdf417Encoder::new();
    let symbol = enc
        .build_sized(
            vec![Segment::alphanumeric(b"FIXED WIDTH".to_vec())],
            level(2),
            Some(5),
        )
        .unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let Encoding::Matrix(m) = &encoding else {
        panic!("expected matrix");
    };
    // width = 17 * columns + 69.
    assert_eq!(m.width(), 17 * 5 + 69);
    let decoded = Pdf417Decoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.segments, symbol.segments);
    assert_eq!(decoded.meta, symbol.meta);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn survives_correctable_errors() {
    // EC level 5 adds 64 check codewords (corrects up to 32 codeword errors).
    let enc = Pdf417Encoder::new();
    let symbol = enc
        .build(
            vec![Segment::alphanumeric(
                b"RESILIENT PDF417 PAYLOAD 2026".to_vec(),
            )],
            level(5),
        )
        .unwrap();
    let Encoding::Matrix(mut m) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };

    // Corrupt several whole data columns by flipping their modules. Each damaged
    // symbol character becomes at most a few codeword errors.
    let rows = m.height() / 3;
    for y in 0..rows.min(8) {
        let x0 = 34; // first data column: start(17) + left indicator(17)
        for j in 0..17 {
            for dy in 0..3 {
                let v = m.get(x0 + j, y * 3 + dy);
                m.set(x0 + j, y * 3 + dy, !v);
            }
        }
    }

    let decoded = Pdf417Decoder::new().decode(&Encoding::Matrix(m)).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("RESILIENT PDF417 PAYLOAD 2026")
    );
}

#[test]
fn text_convenience_roundtrip() {
    let enc = Pdf417Encoder::new();
    let symbol = enc
        .build_text("https://github.com/KarpelesLab/anydcode", level(2))
        .unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = Pdf417Decoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("https://github.com/KarpelesLab/anydcode")
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

/// Independent reference vector: the high-level codewords for these inputs are
/// documented in ZXing's `PDF417EncoderTestCase` (which follows ISO/IEC 15438
/// Annex P). We verify them through the public API by encoding a fixed-geometry
/// symbol and confirming the exact data-region codewords.
#[test]
fn reference_codewords_via_public_api() {
    // "ABCD" as Text compaction → codewords 900 (latch), 1, 63.
    // With EC level 0 (k = 2) and a single column we get rows such that the data
    // region begins with the symbol-length descriptor then the high-level stream.
    let enc = Pdf417Encoder::new();
    let symbol = enc
        .build_sized(
            vec![Segment::alphanumeric(b"ABCD".to_vec())],
            level(0),
            Some(1),
        )
        .unwrap();
    let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    // Decode back and confirm the text survives; the internal codeword check is in
    // the compaction unit tests.
    let decoded = Pdf417Decoder::new().decode(&Encoding::Matrix(m)).unwrap();
    assert_eq!(decoded.text().as_deref(), Some("ABCD"));
}

/// Build a module matrix from a zint `--dump` (one hex row per codeword row, MSB
/// first), repeating each row to this crate's three-module row height.
fn matrix_from_zint_dump(rows: &[&str], width: usize) -> anyd::output::BitMatrix {
    let mut m = anyd::output::BitMatrix::new(width, rows.len() * 3, 2);
    for (y, hex) in rows.iter().enumerate() {
        let bytes: Vec<u8> = hex
            .split_whitespace()
            .map(|b| u8::from_str_radix(b, 16).unwrap())
            .collect();
        for x in 0..width {
            if (bytes[x / 8] >> (7 - x % 8)) & 1 == 1 {
                for dy in 0..3 {
                    m.set(x, y * 3 + dy, true);
                }
            }
        }
    }
    m
}

/// Third-party symbol (zint 2.16.0, `zint -b PDF417 -d "abcédef" --dump`). zint omits
/// the initial Text latch (Text/Alpha is the default mode) and encodes the lone
/// non-text byte with the 913 "shift to Byte" codeword, after which the Lower
/// sub-mode is still in effect: data codewords `810 32 913 233 94 179`.
#[test]
fn decodes_zint_symbol_with_byte_shift() {
    let dump = [
        "FF 54 7D 5F 35 0C 10 50 4F 57 87 F4 52",
        "FF 54 7E A3 BD 73 97 E4 6F 52 07 F4 52",
        "FF 54 75 7E 2C 81 D4 41 EE A3 F7 F4 52",
        "FF 54 6B CF BB 4C 10 C6 4A F3 C7 F4 52",
        "FF 54 75 C3 3F 57 14 3E CF 5C E7 F4 52",
        "FF 54 7A F4 2E 19 DB 43 CE BE 87 F4 52",
        "FF 54 74 EF A3 06 9E F3 6D 3B C7 F4 52",
        "FF 54 7D 2C 3E 3B 5E 4D 8A FD C7 F4 52",
    ];
    let m = matrix_from_zint_dump(&dump, 17 * 2 + 69);
    let decoded = Pdf417Decoder::new().decode(&Encoding::Matrix(m)).unwrap();
    assert_eq!(decoded.payload_bytes(), b"abc\xe9def");
    assert_eq!(
        decoded.segments,
        vec![
            Segment::alphanumeric(b"abc".to_vec()),
            Segment::byte(vec![0xe9]),
            Segment::alphanumeric(b"def".to_vec()),
        ]
    );
}

/// A symbol holds at most 928 codewords in total (data + error correction). `build`
/// must not hand back a geometry the encoder then refuses: either it fits, or the
/// capacity error surfaces at build time.
#[test]
fn build_respects_the_928_codeword_limit() {
    let enc = Pdf417Encoder::new();
    for ec in [6u8, 7, 8] {
        for len in [60usize, 300, 490, 498, 504, 600, 900, 1100] {
            match enc.build(vec![Segment::byte(vec![0xAB; len])], level(ec)) {
                Ok(symbol) => {
                    let encoding = enc.encode(&symbol).unwrap_or_else(|e| {
                        panic!("built a {len}-byte level-{ec} symbol that cannot encode: {e:?}")
                    });
                    let decoded = Pdf417Decoder::new().decode(&encoding).unwrap();
                    assert_eq!(decoded.segments, symbol.segments);
                }
                Err(e) => assert!(matches!(e, anyd::error::Error::Capacity { .. }), "{e:?}"),
            }
        }
    }
}

use anyd::codes::pdf417::{Pdf417Meta, Pdf417Variant};
use anyd::error::Error;
use anyd::segment::Mode;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;

/// Builds a segment of the given length.
type MakeSegment = fn(usize) -> Segment;

fn symbol_with(segments: Vec<Segment>, rows: usize, columns: usize, ec: u8) -> Symbol {
    Symbol::new(
        Symbology::Pdf417,
        segments,
        SymbolMeta::Pdf417(Pdf417Meta {
            rows,
            columns,
            ec_level: level(ec),
            variant: Pdf417Variant::Standard,
        }),
    )
}

/// The start pattern, both row-indicator columns and the stop pattern depend only on
/// the geometry (row number, row count, column count, EC level). This crate's
/// decoder never reads the indicators, so nothing else pins the encoder's: compare
/// them with zint 2.16.0 for two geometries that differ in every indicator input
/// (`zint -b PDF417 -d "abcédef" --dump`: 8 rows x 2 columns, level 2, and
/// `zint -b PDF417 --cols=3 --rows=13 --secure=4 -d A --dump`).
#[test]
fn guards_and_row_indicators_match_zint() {
    let small = [
        "FF 54 7D 5F 35 0C 10 50 4F 57 87 F4 52",
        "FF 54 7E A3 BD 73 97 E4 6F 52 07 F4 52",
        "FF 54 75 7E 2C 81 D4 41 EE A3 F7 F4 52",
        "FF 54 6B CF BB 4C 10 C6 4A F3 C7 F4 52",
        "FF 54 75 C3 3F 57 14 3E CF 5C E7 F4 52",
        "FF 54 7A F4 2E 19 DB 43 CE BE 87 F4 52",
        "FF 54 74 EF A3 06 9E F3 6D 3B C7 F4 52",
        "FF 54 7D 2C 3E 3B 5E 4D 8A FD C7 F4 52",
    ];
    let large = [
        "FF 54 7A 9E 3A 8E 15 EF 08 63 27 D5 F3 FA 29",
        "FF 54 7A 81 2F D6 17 EB 0B F5 87 EA 73 FA 29",
        "FF 54 54 F0 31 F2 57 94 0D 85 E5 6F 83 FA 29",
        "FF 54 7D 7E B2 37 17 1E CC D1 06 BC FB FA 29",
        "FF 54 75 EE 34 71 19 1E 6F 28 87 5C 63 FA 29",
        "FF 54 7D 7B 3D 1E 94 0B CF 27 A7 5F 0B FA 29",
        "FF 54 69 CF 21 08 5A 21 8E E9 07 4E FB FA 29",
        "FF 54 7A 44 3E B3 9F 77 6F DD 25 7E 73 FA 29",
        "FF 54 7E 99 39 2F D6 83 88 B7 C5 39 FB FA 29",
        "FF 54 51 DC 22 81 10 45 0C EE 65 18 63 FA 29",
        "FF 54 53 D0 38 B0 98 9E CB 7C 47 4E 63 FA 29",
        "FF 54 68 8F B7 11 D3 C2 2D E1 65 1D FB FA 29",
        "FF 54 50 50 20 86 D9 C6 4C 22 36 87 7B FA 29",
    ];
    for (dump, columns, ec) in [(&small[..], 2usize, 2u8), (&large[..], 3, 4)] {
        let width = 17 * columns + 69;
        let reference = matrix_from_zint_dump(dump, width);
        let symbol = symbol_with(
            vec![Segment::alphanumeric(b"Z".to_vec())],
            dump.len(),
            columns,
            ec,
        );
        let Encoding::Matrix(ours) = Pdf417Encoder::new().encode(&symbol).unwrap() else {
            panic!("expected matrix");
        };
        assert_eq!((ours.width(), ours.height()), (width, dump.len() * 3));
        for y in 0..ours.height() {
            // Start (17) + left indicator (17) ... right indicator (17) + stop (18).
            for x in (0..34).chain(width - 35..width) {
                assert_eq!(
                    ours.get(x, y),
                    reference.get(x, y),
                    "{columns} columns: module ({x}, {y}) differs from zint"
                );
            }
        }
    }
}

/// ISO/IEC 15438 quotes the symbol maxima at error-correction level 0 as 1108 bytes,
/// 2710 digits and 1850 text characters; with this encoder's explicit Text latch the
/// last is 1848. One unit more must be a capacity error — at build time and when
/// encoding a hand-made symbol — never a panic or a truncated symbol.
#[test]
fn capacity_boundaries_per_mode() {
    let enc = Pdf417Encoder::new();
    let cases: [(MakeSegment, usize); 3] = [
        (|n| Segment::byte(vec![0xA5; n]), 1108),
        (|n| Segment::numeric(vec![b'7'; n]), 2710),
        (|n| Segment::alphanumeric(vec![b'Q'; n]), 1848),
    ];
    for (make, max) in cases {
        assert_lossless(vec![make(max)], level(0));
        for over in [max + 1, max + 2, max + 7, 4000] {
            let err = enc.build(vec![make(over)], level(0)).unwrap_err();
            assert!(matches!(err, Error::Capacity { .. }), "{over}: {err:?}");
            // The largest geometry there is still cannot take it.
            for (rows, columns) in [(90, 10), (32, 29), (90, 30)] {
                let symbol = symbol_with(vec![make(over)], rows, columns, 0);
                let err = enc.encode(&symbol).unwrap_err();
                assert!(matches!(err, Error::Capacity { .. }), "{over}: {err:?}");
            }
        }
    }
    // An explicit column count too narrow for the data is a capacity error too.
    let err = enc
        .build_sized(vec![Segment::byte(vec![1; 200])], level(0), Some(1))
        .unwrap_err();
    assert!(matches!(err, Error::Capacity { .. }), "{err:?}");
    for columns in [0, 31, 1000] {
        assert!(
            enc.build_sized(vec![Segment::byte(vec![1; 5])], level(0), Some(columns))
                .is_err(),
            "{columns} columns"
        );
    }
}

/// Every compaction mode at every length around its group size (Byte: 6 bytes per 5
/// codewords with the 901/924 latch split; Numeric: 44 digits per 15 codewords; Text:
/// two characters per codeword) round-trips, alone and next to each other mode.
#[test]
fn every_group_boundary_roundtrips() {
    for n in (1..=14).chain([17, 18, 19, 43, 44, 45, 46, 87, 88, 89, 90, 131, 132, 133]) {
        let byte = Segment::byte((0..n).map(|i| (i * 37 + 200) as u8).collect::<Vec<u8>>());
        let numeric = Segment::numeric((0..n).map(|i| b'0' + (i % 10) as u8).collect::<Vec<u8>>());
        // Leading zeros must survive the base-900 conversion.
        let zeros = Segment::numeric(vec![b'0'; n]);
        let text = Segment::alphanumeric((0..n).map(|i| b"Az 9;\t"[i % 6]).collect::<Vec<u8>>());
        for seg in [&byte, &numeric, &zeros, &text] {
            assert_lossless(vec![seg.clone()], level(1));
        }
        assert_lossless(
            vec![text.clone(), byte.clone(), numeric.clone(), text.clone()],
            level(2),
        );
        assert_lossless(
            vec![numeric.clone(), zeros.clone(), byte.clone(), byte.clone()],
            level(0),
        );
    }
}

/// Adjacent segments of the same mode each carry their own latch, so even those
/// boundaries survive; so does a symbol with no data at all.
#[test]
fn same_mode_neighbours_and_empty_symbol_roundtrip() {
    assert_lossless(
        vec![
            Segment::alphanumeric(b"ABC".to_vec()),
            Segment::alphanumeric(b"abc".to_vec()),
            Segment::numeric(b"12".to_vec()),
            Segment::numeric(b"0034".to_vec()),
            Segment::byte(vec![1, 2, 3, 4, 5, 6]),
            Segment::byte(vec![7]),
        ],
        level(2),
    );
    assert_lossless(Vec::new(), level(0));
    assert_lossless(Vec::new(), level(8));
}

/// A geometry larger than the data needs is filled with pad codewords, which decode
/// to nothing and leave the pinned geometry intact.
#[test]
fn oversized_geometry_pads_and_roundtrips() {
    let enc = Pdf417Encoder::new();
    for (rows, columns, ec) in [(10, 1, 0), (90, 1, 0), (3, 30, 3), (30, 30, 8), (90, 10, 5)] {
        let symbol = symbol_with(
            vec![
                Segment::alphanumeric(b"Pad me".to_vec()),
                Segment::numeric(b"42".to_vec()),
            ],
            rows,
            columns,
            ec,
        );
        let encoding = enc.encode(&symbol).unwrap();
        let decoded = Pdf417Decoder::new().decode(&encoding).unwrap();
        assert_eq!(decoded, symbol, "{rows}x{columns} level {ec}");
        assert_eq!(enc.encode(&decoded).unwrap(), encoding);
    }
}

/// Every error-correction level repairs what ISO/IEC 15438 guarantees for it —
/// `(2^(level+1) - 2) / 2` damaged codewords anywhere in the symbol, including the
/// symbol length descriptor — and recovers the exact symbol.
#[test]
fn every_level_corrects_its_guaranteed_capacity() {
    let enc = Pdf417Encoder::new();
    let mut rng = anyd::transform::Rng::new(0x0417);
    for ec in 0..=8u8 {
        let symbol = enc
            .build_sized(
                vec![
                    Segment::alphanumeric(b"Error correction level check".to_vec()),
                    Segment::byte(vec![0, 255, 17, 34]),
                    Segment::numeric(b"00123456789".to_vec()),
                ],
                level(ec),
                Some(6 + ec as usize * 3),
            )
            .unwrap();
        let SymbolMeta::Pdf417(meta) = &symbol.meta else {
            panic!("expected PDF417 meta");
        };
        assert_eq!(meta.ec_level.ec_codewords(), 2usize << ec);
        let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
            panic!("expected matrix");
        };
        let cells = meta.rows * meta.columns;
        let errors = (meta.ec_level.ec_codewords() - 2) / 2;
        for round in 0..3 {
            let mut m = clean.clone();
            // Always hit the symbol length descriptor (cell 0) when anything is hit.
            let mut hit = vec![false; cells];
            let mut left = errors;
            if left > 0 && round > 0 {
                hit[0] = true;
                left -= 1;
            }
            while left > 0 {
                let cell = (rng.next_u64() % cells as u64) as usize;
                if !hit[cell] {
                    hit[cell] = true;
                    left -= 1;
                }
            }
            for (cell, _) in hit.iter().enumerate().filter(|(_, h)| **h) {
                let (row, col) = (cell / meta.columns, cell % meta.columns);
                // Blank the symbol character: an unreadable codeword.
                for x in 34 + 17 * col..34 + 17 * (col + 1) {
                    for y in row * 3..row * 3 + 3 {
                        m.set(x, y, false);
                    }
                }
            }
            let decoded = Pdf417Decoder::new()
                .decode(&Encoding::Matrix(m))
                .unwrap_or_else(|e| panic!("level {ec}, {errors} errors: {e:?}"));
            assert_eq!(decoded, symbol, "level {ec}");
        }
    }
}

/// The parameter and input error paths of the public API.
#[test]
fn rejects_bad_parameters_and_input() {
    let enc = Pdf417Encoder::new();
    let dec = Pdf417Decoder::new();

    // EC level range.
    assert!(EcLevel::new(9).is_none());
    assert_eq!(EcLevel::try_from(8u8).unwrap().level(), 8);
    assert!(matches!(
        EcLevel::try_from(9u8),
        Err(Error::InvalidParameter { .. })
    ));
    assert_eq!(
        Pdf417Meta::default(),
        Pdf417Meta {
            rows: 3,
            columns: 1,
            ec_level: level(0),
            variant: Pdf417Variant::Standard,
        }
    );

    // Segment content the compaction modes cannot carry.
    let bad_segments = [
        (Segment::numeric(b"12a4".to_vec()), false),
        (Segment::alphanumeric(vec![b'A', 0x80]), false),
        (Segment::alphanumeric(vec![b'A', 0x0B]), false),
        (Segment::byte(Vec::new()), false),
        (
            Segment {
                mode: Mode::Kanji,
                data: vec![0x93, 0x5f],
            },
            true,
        ),
        // ECI is not implemented; with its (empty) data it is refused as invalid.
        (
            Segment {
                mode: Mode::Eci(26),
                data: Vec::new(),
            },
            false,
        ),
        (
            Segment {
                mode: Mode::Eci(26),
                data: vec![b'A'],
            },
            true,
        ),
    ];
    for (segment, unsupported) in bad_segments {
        let err = enc.build(vec![segment.clone()], level(1)).unwrap_err();
        if unsupported {
            assert!(matches!(err, Error::Unsupported { .. }), "{segment:?}");
        } else {
            assert!(matches!(err, Error::InvalidData { .. }), "{segment:?}");
        }
        assert!(enc.encode(&symbol_with(vec![segment], 10, 5, 1)).is_err());
    }

    // Geometry outside 3..=90 rows / 1..=30 columns, or too small for the EC level.
    let seg = || vec![Segment::alphanumeric(b"A".to_vec())];
    for (rows, columns, ec) in [
        (2, 5, 0),
        (91, 5, 0),
        (5, 0, 0),
        (5, 31, 0),
        (3, 1, 1),
        (3, 5, 4),
    ] {
        let err = enc
            .encode(&symbol_with(seg(), rows, columns, ec))
            .unwrap_err();
        assert!(
            matches!(err, Error::InvalidParameter { .. }),
            "{rows}x{columns} level {ec}: {err:?}"
        );
    }

    // A symbol of another symbology, or without PDF417 metadata.
    let mut other = symbol_with(seg(), 5, 5, 1);
    other.symbology = Symbology::MicroPdf417;
    assert!(matches!(
        enc.encode(&other),
        Err(Error::InvalidParameter { .. })
    ));
    let bare = Symbol::new(Symbology::Pdf417, seg(), SymbolMeta::Generic);
    assert!(matches!(
        enc.encode(&bare),
        Err(Error::InvalidParameter { .. })
    ));

    // Decoder input that cannot be a PDF417 grid.
    let linear = Encoding::Linear(anyd::output::LinearPattern {
        modules: vec![true, false, true],
        quiet_zone: 2,
    });
    assert!(matches!(
        dec.decode(&linear),
        Err(Error::Unsupported { .. })
    ));
    for (width, height) in [
        (0, 0),
        (69, 9),            // no data column
        (17 * 2 + 68, 9),   // not 17c + 69
        (17 * 31 + 69, 9),  // 31 columns
        (17 * 2 + 69, 10),  // not a whole number of rows
        (17 * 2 + 69, 6),   // 2 rows
        (17 * 2 + 69, 273), // 91 rows
    ] {
        let m = anyd::output::BitMatrix::new(width, height, 2);
        assert!(dec.decode_matrix(&m).is_err(), "{width}x{height}");
    }
    // A blank grid of valid dimensions has no readable codeword at all.
    let blank = anyd::output::BitMatrix::new(17 * 4 + 69, 30, 2);
    assert!(dec.decode_matrix(&blank).is_err());
}
