//! End-to-end Grid Matrix (AIMD014 / GB/T 27766) round-trip tests.
//!
//! Every payload must encode → decode to the same segments/metadata and re-encode
//! byte-identically, across modes, versions, and EC levels, and must survive
//! correctable errors. A structural reference test cross-checks the version-1 frame
//! geometry against zint's documented macromodule layout.

use anyd::codes::gridmatrix::{EcLevel, GmMode, GridMatrixDecoder, GridMatrixEncoder, Version};
use anyd::output::Encoding;
use anyd::symbol::SymbolMeta;
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
