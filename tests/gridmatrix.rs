//! End-to-end Grid Matrix (AIMD014 / GB/T 27766) round-trip tests.
//!
//! Every payload must encode → decode to the same segments/metadata and re-encode
//! byte-identically, across modes, versions, and EC levels, and must survive
//! correctable errors. A structural reference test cross-checks the version-1 frame
//! geometry against zint's documented macromodule layout.
#![cfg(all(feature = "decode", feature = "encode", feature = "gridmatrix"))]

use anyd::codes::gridmatrix::{
    EcLevel, GmMode, GridMatrixDecoder, GridMatrixEncoder, GridMatrixMeta, Version,
};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::{Symbol, SymbolMeta};
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Assert a payload decodes to the same segments/meta and re-encodes identically.
fn assert_lossless(data: &[u8]) {
    let enc = GridMatrixEncoder::new();
    let symbol = enc.build(data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = GridMatrixDecoder::new().decode(&encoding).unwrap();
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
fn numeric() {
    assert_lossless(b"123456");
    assert_lossless(b"0000000000");
    assert_lossless(b"7"); // single digit
    assert_lossless(b"12345678901234567890");
}

#[test]
fn upper_and_space() {
    assert_lossless(b"HELLO WORLD");
    assert_lossless(b"GRID MATRIX CODE");
    assert_lossless(b"A");
}

#[test]
fn lower_and_space() {
    assert_lossless(b"hello world");
    assert_lossless(b"grid matrix");
}

#[test]
fn byte_arbitrary() {
    assert_lossless(&[0x00, 0x80, 0xFF, 0xAB, 0xCD]);
    assert_lossless(&(0u16..=255).map(|b| b as u8).collect::<Vec<u8>>());
    assert_lossless(b"http://example.com/anydcode?x=1&y=2");
}

#[test]
fn mixed_mode_sequences() {
    assert_lossless(b"ABC123def456!!");
    assert_lossless(b"Order 42 items for $9");
    let mut data = b"USER".to_vec();
    data.extend_from_slice(&[0x80, 0x90]);
    data.extend_from_slice(b"name99");
    assert_lossless(&data);
}

#[test]
fn empty_payload() {
    assert_lossless(b"");
}

#[test]
fn segment_modes_are_recovered() {
    let enc = GridMatrixEncoder::new();
    // "AB" (upper) then "12" (numeral) then "cd" (lower) then a high byte.
    let mut data = b"AB12cd".to_vec();
    data.push(0x80);
    let symbol = enc.build(&data).unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = GridMatrixDecoder::new().decode(&encoding).unwrap();
    let SymbolMeta::GridMatrix(meta) = &decoded.meta else {
        panic!("expected Grid Matrix meta");
    };
    assert_eq!(
        meta.modes,
        vec![GmMode::Upper, GmMode::Numeral, GmMode::Lower, GmMode::Byte]
    );
    assert_eq!(decoded.segments.len(), 4);
}

#[test]
fn spans_many_versions() {
    // Growing payloads force progressively larger symbols across the version range.
    for n in [1usize, 10, 40, 100, 250, 600, 1200] {
        let data: Vec<u8> = (0..n).map(|i| b'0' + (i % 10) as u8).collect();
        assert_lossless(&data);
    }
}

#[test]
fn all_ec_levels_round_trip() {
    let enc = GridMatrixEncoder::new();
    for ec in EcLevel::ALL {
        let symbol = enc.build_with_ec(b"GRID MATRIX 2026", ec).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        let decoded = GridMatrixDecoder::new().decode(&encoding).unwrap();
        assert_eq!(decoded.text().as_deref(), Some("GRID MATRIX 2026"));
        let SymbolMeta::GridMatrix(meta) = &decoded.meta else {
            panic!("expected Grid Matrix meta");
        };
        assert_eq!(meta.ec_level, ec, "EC level not recovered");
        assert_eq!(enc.encode(&decoded).unwrap(), encoding);
    }
}

#[test]
fn forced_version_and_ec() {
    let enc = GridMatrixEncoder::new();
    let v = Version::new(3).unwrap();
    let symbol = enc.build_sized(b"12", v, EcLevel::L3).unwrap();
    let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!((m.width(), m.height()), (v.size(), v.size()));
    assert_eq!(v.size(), 42);
    let decoded = GridMatrixDecoder::new()
        .decode(&Encoding::Matrix(m.clone()))
        .unwrap();
    assert_eq!(decoded.payload_bytes(), b"12");
    assert_eq!(enc.encode(&decoded).unwrap(), Encoding::Matrix(m));
}

#[test]
fn survives_correctable_errors() {
    let enc = GridMatrixEncoder::new();
    // Version 5 at L1 (strongest EC) has two RS blocks of 12 EC codewords each,
    // correcting up to 6 errors per block. Corrupt a handful of interior data
    // modules and expect full recovery.
    let v = Version::new(5).unwrap();
    let symbol = enc
        .build_sized(b"RESILIENT GRID MATRIX DATA", v, EcLevel::L1)
        .unwrap();
    let Encoding::Matrix(mut matrix) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    // Flip one interior data module in each of four distinct macromodules (cols/rows
    // 1..4 within a 6x6 block are data; avoid the frame perimeter at 6k and 6k+5).
    for k in 0..4 {
        let x = 6 * k + 2;
        let y = 6 * k + 3;
        let v = matrix.get(x, y);
        matrix.set(x, y, !v);
    }
    let decoded = GridMatrixDecoder::new()
        .decode(&Encoding::Matrix(matrix))
        .unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("RESILIENT GRID MATRIX DATA")
    );
}

/// Independent structural cross-check against zint's `gridmtx.c`.
///
/// zint draws a solid 6×6 frame for every macromodule where `(x + y)` is even. For a
/// version-1 symbol (3×3 macromodules, 18×18 modules) that makes the four corner
/// macromodules and the centre framed, and the four edge-midpoint macromodules
/// unframed. We verify the top-left macromodule's frame perimeter is dark while the
/// macromodule to its right (an odd-parity block) has a light perimeter.
#[test]
fn version1_frame_matches_zint_layout() {
    let enc = GridMatrixEncoder::new();
    let symbol = enc.build_with_ec(b"1", EcLevel::L5).unwrap();
    let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
        panic!("expected matrix");
    };
    assert_eq!((m.width(), m.height()), (18, 18));

    // Macromodule (0,0) is framed: its full 6x6 perimeter is dark.
    for i in 0..6 {
        assert!(m.get(i, 0), "top edge ({i},0) of framed macromodule");
        assert!(m.get(i, 5), "bottom edge ({i},5) of framed macromodule");
        assert!(m.get(0, i), "left edge (0,{i}) of framed macromodule");
        assert!(m.get(5, i), "right edge (5,{i}) of framed macromodule");
    }

    // Macromodule (1,0) has odd parity → unframed: its perimeter is light. Its top row
    // (y=0) spans columns 6..=11; those frame cells are never set.
    for x in 6..=11 {
        assert!(
            !m.get(x, 0),
            "unframed macromodule top edge ({x},0) is light"
        );
        assert!(
            !m.get(x, 5),
            "unframed macromodule bottom edge ({x},5) is light"
        );
    }
}

// ---- Third-party reference vectors --------------------------------------------
//
// 18x18 (version 1) matrices produced by zint 2.16 (`zint -b GRIDMATRIX --dump`), one
// string per row, '1' = dark: "1234567" (numeral mode, short final group) and the bytes
// 80 81 FE (byte mode).

#[rustfmt::skip]
const ZINT_NUMERAL_1234567: &[&str] = &[
    "111111000000111111",
    "101101001010101011",
    "111001000010111111",
    "101111001100111011",
    "111101011100111101",
    "111111000000111111",
    "000000111111000000",
    "001110100001001000",
    "000110111111000000",
    "001010100011001010",
    "010000101001000000",
    "000000111111000000",
    "111111000000111111",
    "101001001000101111",
    "101101001110110101",
    "111001000010100001",
    "111011001010100001",
    "111111000000111111",
];
#[rustfmt::skip]
const ZINT_BYTES_80_81_FE: &[&str] = &[
    "111111000000111111",
    "101011001100101101",
    "110111000000100001",
    "110001000000101111",
    "100111000000111111",
    "111111000000111111",
    "000000111111000000",
    "001100100001001110",
    "001100100101011110",
    "010110110111000000",
    "001010100001000000",
    "000000111111000000",
    "111111000000111111",
    "101011001000101001",
    "101111010110111001",
    "101101010110100001",
    "101111011100100001",
    "111111000000111111",
];

/// A zint symbol must decode to `payload`, and both re-encoding the decoded symbol
/// and building `payload` from scratch must reproduce zint's matrix exactly.
fn assert_matches_zint(rows: &[&str], payload: &[u8], modes: &[GmMode]) {
    let mut m = anyd::output::BitMatrix::new(rows[0].len(), rows.len(), 2);
    for (y, row) in rows.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            m.set(x, y, c == b'1');
        }
    }
    let reference = Encoding::Matrix(m);
    let decoded = GridMatrixDecoder::new().decode(&reference).unwrap();
    assert_eq!(decoded.payload_bytes(), payload);
    let SymbolMeta::GridMatrix(meta) = &decoded.meta else {
        panic!("expected GridMatrixMeta");
    };
    assert_eq!(meta.modes, modes);
    let enc = GridMatrixEncoder::new();
    assert_eq!(enc.encode(&decoded).unwrap(), reference);
    let built = enc
        .build_sized(payload, meta.version, meta.ec_level)
        .unwrap();
    assert_eq!(enc.encode(&built).unwrap(), reference);
}

#[test]
fn numeral_mode_matches_zint() {
    assert_matches_zint(ZINT_NUMERAL_1234567, b"1234567", &[GmMode::Numeral]);
}

#[test]
fn byte_mode_matches_zint() {
    assert_matches_zint(ZINT_BYTES_80_81_FE, &[0x80, 0x81, 0xFE], &[GmMode::Byte]);
}

#[test]
fn decoder_never_panics_on_garbage() {
    // Deterministic xorshift noise: random grids of every valid (and some invalid)
    // size, then valid symbols with growing numbers of flipped modules.
    let mut seed = 0x6A1D_3A7A_1C5E_ED01u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let dec = GridMatrixDecoder::new();
    let mut sizes: Vec<usize> = (1..=14).map(|v| 6 + 12 * v).collect();
    sizes.extend([0, 1, 6, 17, 19, 24]);
    for size in sizes {
        for density in [0u64, 1, 2, 3, 4] {
            for _ in 0..3 {
                let mut m = anyd::output::BitMatrix::new(size, size, 2);
                for y in 0..size {
                    for x in 0..size {
                        m.set(x, y, next() % 4 < density);
                    }
                }
                let _ = dec.decode(&Encoding::Matrix(m));
            }
        }
    }
    assert!(
        dec.decode(&Encoding::Matrix(anyd::output::BitMatrix::new(18, 30, 2)))
            .is_err()
    );

    let enc = GridMatrixEncoder::new();
    for len in [0usize, 1, 9, 40, 200, 400] {
        let payload: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let symbol = enc.build(&payload).unwrap();
        let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
            panic!("expected matrix");
        };
        let size = clean.width();
        for flips in [1usize, 10, 100, 1000] {
            let mut m = clean.clone();
            for _ in 0..flips {
                let (x, y) = (
                    (next() % size as u64) as usize,
                    (next() % size as u64) as usize,
                );
                let dark = m.get(x, y);
                m.set(x, y, !dark);
            }
            let result = dec.decode(&Encoding::Matrix(m));
            // One module is at most one codeword error, within every level's capacity.
            if flips == 1 {
                assert_eq!(result.unwrap(), symbol, "len {len}");
            }
        }
    }
}

// --- Larger zint vectors: several layers, several RS blocks, mode switches ---

/// A matrix from zint's `--dump` rows (hex, most significant bit first).
fn matrix_from_hex(rows: &[&str]) -> anyd::output::BitMatrix {
    let size = rows.len();
    let mut m = anyd::output::BitMatrix::new(size, size, 2);
    for (y, row) in rows.iter().enumerate() {
        for x in 0..size {
            let nibble = (row.as_bytes()[x / 4] as char).to_digit(16).unwrap();
            m.set(x, y, nibble & (8 >> (x % 4)) != 0);
        }
    }
    m
}

/// `zint -b 142 --vers=3 --secure=2 -d "GRID MATRIX LAYERS AND BLOCKS"` (42×42):
/// three layers of layer ids and upper-case mode with spaces.
#[test]
fn version_3_upper_mode_matches_zint() {
    let reference = Encoding::Matrix(matrix_from_hex(&[
        "FC0FC0FC0FC",
        "DD6DD6DD6DC",
        "C5EFDEFDEFC",
        "9C084084084",
        "D4084084084",
        "FC0FC0FC0FC",
        "03F03F03F00",
        "4AF22D3AF58",
        "4BF13B7BF78",
        "6A11BB02100",
        "1A13A902100",
        "03F03F03F00",
        "FC0FC0FC0FC",
        "D4E84284EDC",
        "ADE95C95EFC",
        "DC0FD48C084",
        "F40D40C4084",
        "FC0FC0FC0FC",
        "03F03F03F00",
        "5AF1BD0AF58",
        "3BF2310BF78",
        "7A10353A100",
        "3A10230A100",
        "03F03F03F00",
        "FC0FC0FC0FC",
        "DCE8C084EDC",
        "FDEB44DDEFC",
        "D40E40BC084",
        "940B4CEC084",
        "FC0FC0FC0FC",
        "03F03F03F00",
        "42F3AF3AF58",
        "7BF7BF7BF78",
        "5A102102100",
        "32102102100",
        "03F03F03F00",
        "FC0FC0FC0FC",
        "CD0C50DD6DC",
        "94CDD6FDEFC",
        "B50A5084084",
        "8CAE4084084",
        "FC0FC0FC0FC",
    ]));
    let enc = GridMatrixEncoder::new();
    let payload = b"GRID MATRIX LAYERS AND BLOCKS";
    let decoded = GridMatrixDecoder::new().decode(&reference).unwrap();
    let built = enc
        .build_sized(payload, Version::new(3).unwrap(), EcLevel::L2)
        .unwrap();
    assert_eq!(decoded, built);
    assert_eq!(enc.encode(&built).unwrap(), reference);
}

/// `zint -b 142 --vers=4 --secure=4 --binary --esc -d
/// "\x80\x81\xFE\xFF\x00\x01 bytes \xA5…(27×)"` (54×54): byte → lower → byte, so
/// the 4-bit switch out of byte mode and the 7-bit switch into it are both
/// checked, across several interleaved RS blocks.
#[test]
fn version_4_byte_and_lower_switches_match_zint() {
    let reference = Encoding::Matrix(matrix_from_hex(&[
        "FC0FC0FC0FC0FC",
        "BCEB4EB4CB4AAC",
        "DD6BDED44D52EC",
        "F48CD6B44ED6EC",
        "DCAFD2A52CC69C",
        "FC0FC0FC0FC0FC",
        "03F03F03F03F00",
        "32702702702738",
        "73F03F03F03F28",
        "0A10A912B32748",
        "3A13372A54A958",
        "03F03F03F03F00",
        "FC0FC0FC0FC0FC",
        "B40FD8FD8FC0BC",
        "8C0FC0FC0FC0D4",
        "BCA94E840A4AF4",
        "FC0ED4BD2CD4AC",
        "FC0FC0FC0FC0FC",
        "03F03F03F03F00",
        "32763743762720",
        "5BF03F03F03F40",
        "4A72A10210A508",
        "4295295A133B08",
        "03F03F03F03F00",
        "FC0FC0FC0FC0FC",
        "BC0FD0BD0FC0BC",
        "A40FC0FC0FC0B4",
        "84C9C09C8AC2FC",
        "9D2A40840FCCFC",
        "FC0FC0FC0FC0FC",
        "03F03F03F03F00",
        "3A763743762728",
        "0BF03F03F03F70",
        "12B32F2AF22910",
        "5A54A17BF0B758",
        "03F03F03F03F00",
        "FC0FC0FC0FC0FC",
        "A40FD8FD8FC0BC",
        "840FC0FC0FC084",
        "C44AC4A42AC4AC",
        "ECA94ADCCACA8C",
        "FC0FC0FC0FC0FC",
        "03F03F03F03F00",
        "22702702702720",
        "73F03F03F03F40",
        "4A90A52A732B58",
        "33733B5294A558",
        "03F03F03F03F00",
        "FC0FC0FC0FC0FC",
        "B4ABCCB48B4CB4",
        "DD694EA4E940DC",
        "E40ACE956E5AAC",
        "AD6EC6B52FDCA4",
        "FC0FC0FC0FC0FC",
    ]));
    let want = Symbol::new(
        Symbology::GridMatrix,
        vec![
            Segment::byte(b"\x80\x81\xFE\xFF\x00\x01".to_vec()),
            Segment::alphanumeric(b" bytes ".to_vec()),
            Segment::byte(vec![0xA5; 27]),
        ],
        SymbolMeta::GridMatrix(GridMatrixMeta {
            version: Version::new(4).unwrap(),
            ec_level: EcLevel::L4,
            modes: vec![GmMode::Byte, GmMode::Lower, GmMode::Byte],
        }),
    );
    assert_eq!(GridMatrixDecoder::new().decode(&reference).unwrap(), want);
    assert_eq!(GridMatrixEncoder::new().encode(&want).unwrap(), reference);
}

// --- Capacity boundaries ---

/// At every version and level: the longest digit string and byte string the data
/// codewords can hold (numeral mode costs 4 + 2 + 10 bits around 10-bit groups of
/// three digits; a byte block 4 + 9 + 4 around its bytes) fit and round-trip, one
/// more group or byte is a capacity error, and `build_with_ec` moves to the next
/// version exactly there.
#[test]
fn capacity_boundaries_at_every_version_and_level() {
    let enc = GridMatrixEncoder::new();
    let version_of = |s: &Symbol| match &s.meta {
        SymbolMeta::GridMatrix(m) => m.version.number(),
        _ => panic!("expected GridMatrixMeta"),
    };
    for v in 1..=13u8 {
        let version = Version::new(v).unwrap();
        for ec in EcLevel::ALL {
            let bits = 7 * version.data_codewords(ec);
            if bits == 0 {
                // Version 1 has no level 1.
                assert!(enc.build_sized(b"1", version, ec).is_err());
                continue;
            }
            let digits = (bits - 16) / 10 * 3;
            // Every 512 bytes open a new block: 4 + 9 bits each, plus the final 4.
            let byte_bits = |n: usize| 8 * n + 13 * n.div_ceil(512) + 4;
            let bytes = (1..).take_while(|&n| byte_bits(n) <= bits).count();
            for (max, fill) in [(digits, b'7'), (bytes, 0xA5)] {
                let what = format!("v{v} {ec:?} {max} x {fill:#04x}");
                let full = enc.build_sized(&vec![fill; max], version, ec).unwrap();
                let encoding = enc.encode(&full).unwrap();
                assert_eq!(
                    GridMatrixDecoder::new().decode(&encoding).unwrap(),
                    full,
                    "{what}"
                );
                assert_eq!(
                    version_of(&enc.build_with_ec(&vec![fill; max], ec).unwrap()),
                    v,
                    "{what}"
                );

                let over = vec![fill; max + 1];
                assert!(
                    matches!(
                        enc.build_sized(&over, version, ec),
                        Err(anyd::Error::Capacity { .. })
                    ),
                    "{what}"
                );
                match enc.build_with_ec(&over, ec) {
                    Ok(sym) => assert_eq!(version_of(&sym), v + 1, "{what}"),
                    Err(e) => {
                        assert_eq!(v, 13, "{what}");
                        assert!(matches!(e, anyd::Error::Capacity { .. }), "{what}");
                    }
                }
            }
        }
    }
    // `build` gives up past version 13 at its recommended level too.
    assert!(matches!(
        enc.build(&vec![b'7'; 4000]),
        Err(anyd::Error::Capacity { .. })
    ));
}

// --- Error paths and parameter ranges ---

#[test]
fn encoder_rejects_what_it_cannot_represent() {
    let enc = GridMatrixEncoder::new();
    let symbol = |segments: Vec<Segment>, modes: Vec<GmMode>| {
        Symbol::new(
            Symbology::GridMatrix,
            segments,
            SymbolMeta::GridMatrix(GridMatrixMeta {
                version: Version::new(2).unwrap(),
                ec_level: EcLevel::L3,
                modes,
            }),
        )
    };
    let seg = |data: &[u8]| Segment::byte(data.to_vec());
    // Sanity: the helper makes encodable symbols.
    assert!(
        enc.encode(&symbol(vec![seg(b"AB")], vec![GmMode::Upper]))
            .is_ok()
    );
    for (segments, modes) in [
        // Bytes outside the mode's alphabet.
        (vec![seg(b"12a")], vec![GmMode::Numeral]),
        (vec![seg(b"ab")], vec![GmMode::Upper]),
        (vec![seg(b"AB")], vec![GmMode::Lower]),
        (vec![seg(b"a1")], vec![GmMode::Lower]),
        // Runs the bit format cannot express.
        (vec![seg(b"")], vec![GmMode::Numeral]),
        (vec![seg(b"")], vec![GmMode::Byte]),
    ] {
        assert!(
            matches!(
                enc.encode(&symbol(segments, modes.clone())),
                Err(anyd::Error::InvalidData { .. })
            ),
            "{modes:?}"
        );
    }
    for (segments, modes) in [
        // Modes that are modelled but not implemented.
        (vec![seg(b"\xB0\xA1")], vec![GmMode::Chinese]),
        (vec![seg(b"A1")], vec![GmMode::Mixed]),
        // No switch exists from a mode to itself.
        (
            vec![seg(b"A"), seg(b"B")],
            vec![GmMode::Upper, GmMode::Upper],
        ),
        (
            vec![seg(b"\x80"), seg(b"\x81")],
            vec![GmMode::Byte, GmMode::Byte],
        ),
    ] {
        assert!(
            matches!(
                enc.encode(&symbol(segments, modes.clone())),
                Err(anyd::Error::Unsupported { .. })
            ),
            "{modes:?}"
        );
    }
    // Mode list and segment list must line up; and there must be a segment.
    for (segments, modes) in [
        (vec![seg(b"A")], vec![]),
        (vec![seg(b"A")], vec![GmMode::Upper, GmMode::Lower]),
        (vec![], vec![]),
    ] {
        assert!(matches!(
            enc.encode(&symbol(segments, modes)),
            Err(anyd::Error::InvalidParameter { .. })
        ));
    }
    // Wrong symbology / metadata, and version 1 at the level it does not have.
    let mut wrong = symbol(vec![seg(b"A")], vec![GmMode::Upper]);
    wrong.symbology = Symbology::QrCode;
    assert!(matches!(
        enc.encode(&wrong),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    let generic = Symbol::new(Symbology::GridMatrix, vec![seg(b"A")], SymbolMeta::Generic);
    assert!(matches!(
        enc.encode(&generic),
        Err(anyd::Error::InvalidParameter { .. })
    ));
    let mut v1l1 = symbol(vec![seg(b"A")], vec![GmMode::Upper]);
    v1l1.meta = SymbolMeta::GridMatrix(GridMatrixMeta {
        version: Version::new(1).unwrap(),
        ec_level: EcLevel::L1,
        modes: vec![GmMode::Upper],
    });
    assert!(matches!(
        enc.encode(&v1l1),
        Err(anyd::Error::Capacity { .. })
    ));
    assert!(enc.build_with_ec(b"A", EcLevel::L1).is_ok()); // version 2 has level 1
}

#[test]
fn decoder_rejects_non_grid_matrix_inputs() {
    let dec = GridMatrixDecoder::new();
    let linear = Encoding::Linear(anyd::output::LinearPattern::new());
    assert!(matches!(
        dec.decode(&linear),
        Err(anyd::Error::Unsupported { .. })
    ));
}

#[test]
fn parameter_types_validate_their_ranges() {
    assert!(Version::new(0).is_none() && Version::new(14).is_none());
    for v in 1..=13u8 {
        let version = Version::new(v).unwrap();
        assert_eq!(version.number(), v);
        assert_eq!(version.macromodules_per_dim(), 2 * v as usize + 1);
        assert_eq!(version.size(), 6 + 12 * v as usize);
        assert_eq!(version.total_codewords(), 2 * (2 * v as usize + 1).pow(2));
        // More protection, less data; and always some of each.
        let data: Vec<usize> = EcLevel::ALL
            .iter()
            .map(|&ec| version.data_codewords(ec))
            .collect();
        assert!(data.windows(2).all(|w| w[0] > w[1] || w[0] == 0), "v{v}");
        assert!(data[0] < version.total_codewords());
    }
    for (i, ec) in EcLevel::ALL.into_iter().enumerate() {
        assert_eq!(ec.number(), i + 1);
        assert_eq!(EcLevel::from_number(i + 1), Some(ec));
    }
    assert!(EcLevel::from_number(0).is_none() && EcLevel::from_number(6).is_none());
    let modes = [
        GmMode::Chinese,
        GmMode::Numeral,
        GmMode::Lower,
        GmMode::Upper,
        GmMode::Mixed,
        GmMode::Byte,
    ];
    for (i, mode) in modes.into_iter().enumerate() {
        assert_eq!(mode.index() as usize, i + 1);
    }
    let default = GridMatrixMeta::default();
    assert_eq!(
        (
            default.version.number(),
            default.ec_level,
            default.modes.len()
        ),
        (1, EcLevel::L5, 0)
    );
}

/// Random payloads over every character class, at every EC level: `build`,
/// `build_with_ec` and `build_text` all produce symbols that decode to themselves.
#[test]
fn random_payloads_roundtrip_at_every_level() {
    let mut rng = anyd::transform::Rng::new(0x6D61_7472_6978);
    let enc = GridMatrixEncoder::new();
    let dec = GridMatrixDecoder::new();
    for round in 0..120 {
        let len = (rng.next_u64() % 90) as usize;
        let mut payload = Vec::with_capacity(len);
        while payload.len() < len {
            let run = 1 + (rng.next_u64() % 7) as usize;
            let class = rng.next_u64() % 5;
            for _ in 0..run {
                let r = rng.next_u64();
                payload.push(match class {
                    0 => b'0' + (r % 10) as u8,
                    1 => b"ABCXYZ "[(r % 7) as usize],
                    2 => b"abcxyz "[(r % 7) as usize],
                    3 => b"!.,-#\x00\x7F"[(r % 7) as usize],
                    _ => 0x80 | r as u8,
                });
            }
        }
        payload.truncate(len);
        let symbol = match round % 3 {
            0 => enc.build(&payload).unwrap(),
            _ => enc
                .build_with_ec(&payload, EcLevel::ALL[round % 5])
                .unwrap(),
        };
        assert_eq!(symbol.payload_bytes(), payload);
        let encoding = enc.encode(&symbol).unwrap();
        assert_eq!(dec.decode(&encoding).unwrap(), symbol, "{payload:?}");
    }
    let symbol = enc.build_text("Grid Matrix 网格矩阵码 2026").unwrap();
    let decoded = dec.decode(&enc.encode(&symbol).unwrap()).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("Grid Matrix 网格矩阵码 2026")
    );
}
