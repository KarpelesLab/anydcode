//! Aztec Runes (ISO/IEC 24778 Annex A): the fixed 11×11 single-byte symbol.
//!
//! Validation:
//! - The rendered module bitmap for a known value is compared to an independent
//!   reference derived from **zint**'s `AztecCompactMap` and rune algorithm
//!   (`backend/aztec.c` `zint_azrune`), which inverts every other ring bit — equivalent
//!   to XOR-ing each 4-bit GF(16) codeword with `0b1010`, matching **zxing-cpp**
//!   `core/src/aztec/AZDetector.cpp`. Value 0 is order-independent (its whole ring is
//!   the alternating complement pattern); value 42 additionally pins the ring bit
//!   ordering.
//! - encode → decode → re-encode is byte-identical for every value 0..=255.
#![cfg(all(feature = "decode", feature = "encode", feature = "aztec"))]

use anyd::codes::aztec::{AztecDecoder, AztecEncoder};
use anyd::output::{BitMatrix, Encoding};
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

/// Reference 11×11 bitmap for Rune value 0 (rows top→bottom, `#` = dark), derived
/// from zint's `AztecCompactMap`.
const RUNE_0: [&str; 11] = [
    "###.#.#.#.#",
    "###########",
    ".#.......#.",
    "##.#####.##",
    ".#.#...#.#.",
    "##.#.#.#.##",
    ".#.#...#.#.",
    "##.#####.##",
    ".#.......#.",
    ".##########",
    "..#.#.#.#..",
];

/// Reference 11×11 bitmap for Rune value 42, derived the same way.
const RUNE_42: [&str; 11] = [
    "###.......#",
    "###########",
    "##.......#.",
    ".#.#####.#.",
    "##.#...#.#.",
    ".#.#.#.#.#.",
    "##.#...#.##",
    ".#.#####.##",
    "##.......##",
    ".##########",
    "....#.###..",
];

fn matrix_of(value: u8) -> BitMatrix {
    let enc = AztecEncoder::new();
    let symbol = enc.build_rune(value);
    match enc.encode(&symbol).unwrap() {
        Encoding::Matrix(m) => m,
        Encoding::Linear(_) => panic!("Aztec Rune must render as a matrix"),
    }
}

fn assert_matches_reference(value: u8, reference: &[&str; 11]) {
    let m = matrix_of(value);
    assert_eq!(m.width(), 11);
    assert_eq!(m.height(), 11);
    for (y, row) in reference.iter().enumerate() {
        for (x, ch) in row.bytes().enumerate() {
            let expected = ch == b'#';
            assert_eq!(
                m.get(x, y),
                expected,
                "value {value}: module ({x},{y}) differs from reference"
            );
        }
    }
}

#[test]
fn rune_value_0_matches_zint_reference() {
    assert_matches_reference(0, &RUNE_0);
}

#[test]
fn rune_value_42_matches_zint_reference() {
    assert_matches_reference(42, &RUNE_42);
}

#[test]
fn rune_payload_and_meta() {
    let enc = AztecEncoder::new();
    let symbol = enc.build_rune(200);
    assert_eq!(symbol.symbology, Symbology::AztecRunes);
    assert_eq!(symbol.payload_bytes(), vec![200]);
    match &symbol.meta {
        SymbolMeta::Aztec(m) => {
            assert!(m.rune, "meta must flag a Rune");
            assert!(m.compact);
            assert_eq!(m.layers, 0);
        }
        _ => panic!("expected Aztec meta"),
    }
}

#[test]
fn rune_roundtrip_all_values() {
    let enc = AztecEncoder::new();
    let dec = AztecDecoder::new();
    for value in 0u16..=255 {
        let value = value as u8;
        let symbol = enc.build_rune(value);
        let encoding = enc.encode(&symbol).unwrap();

        let decoded = dec.decode(&encoding).unwrap();
        assert_eq!(decoded.symbology, Symbology::AztecRunes);
        assert_eq!(
            decoded.payload_bytes(),
            vec![value],
            "value {value} not recovered"
        );
        assert_eq!(decoded.meta, symbol.meta, "value {value} meta differs");

        let reencoded = enc.encode(&decoded).unwrap();
        assert_eq!(reencoded, encoding, "value {value} re-encode differs");
    }
}

#[test]
fn rune_decode_survives_one_module_error() {
    // The GF(16) Reed–Solomon (5 check words) corrects up to two symbol errors.
    let enc = AztecEncoder::new();
    let Encoding::Matrix(mut m) = enc.encode(&enc.build_rune(99)).unwrap() else {
        panic!("expected a matrix");
    };
    // Flip a single ring data module (top edge, between the corners).
    let v = m.get(4, 0);
    m.set(4, 0, !v);
    let decoded = AztecDecoder::new().decode(&Encoding::Matrix(m)).unwrap();
    assert_eq!(decoded.payload_bytes(), vec![99]);
}

#[test]
fn rune_payload_must_be_one_byte() {
    let enc = AztecEncoder::new();
    let rune = enc.build_rune(7);
    for payload in [&b""[..], b"ab"] {
        let mut bad = rune.clone();
        bad.segments = vec![anyd::segment::Segment::byte(payload.to_vec())];
        assert!(
            matches!(enc.encode(&bad), Err(anyd::Error::InvalidParameter { .. })),
            "payload {payload:?}"
        );
    }
    // The one byte may be split over segments; the metadata is not consulted.
    let mut split = rune.clone();
    split.segments = vec![
        anyd::segment::Segment::byte(Vec::new()),
        anyd::segment::Segment::byte(vec![7]),
    ];
    split.meta = SymbolMeta::Generic;
    assert_eq!(enc.encode(&split).unwrap(), enc.encode(&rune).unwrap());
}

#[test]
fn rune_ring_corrects_two_words_and_rejects_more() {
    // The ring is a GF(16) Reed-Solomon codeword of 2 data + 5 check words, so any
    // two damaged 4-bit words are repaired. Flipping all four bits of a word is the
    // worst damage one word can take; the words lie along the ring four modules at a
    // time, clockwise from the top-left.
    let dec = AztecDecoder::new();
    let ring: Vec<(usize, usize)> = (0..7)
        .map(|i| (2 + i, 0))
        .chain((0..7).map(|i| (10, 2 + i)))
        .chain((0..7).map(|i| (8 - i, 10)))
        .chain((0..7).map(|i| (0, 8 - i)))
        .collect();
    let flip_word = |m: &mut BitMatrix, word: usize| {
        for &(x, y) in &ring[4 * word..4 * word + 4] {
            let v = m.get(x, y);
            m.set(x, y, !v);
        }
    };
    for value in [0u8, 1, 42, 0xA5, 255] {
        let clean = matrix_of(value);
        for a in 0..7 {
            for b in a + 1..7 {
                let mut m = clean.clone();
                flip_word(&mut m, a);
                flip_word(&mut m, b);
                let decoded = dec.decode(&Encoding::Matrix(m)).unwrap();
                assert_eq!(decoded.payload_bytes(), [value], "words {a},{b}");
            }
        }
        // Three damaged words are beyond the code. The decoder changes at most two
        // words, so it cannot get back to the original: it fails, or lands on the
        // codeword of another value.
        let mut m = clean.clone();
        for word in [0, 3, 6] {
            flip_word(&mut m, word);
        }
        match dec.decode(&Encoding::Matrix(m)) {
            Err(_) => {}
            Ok(decoded) => assert_ne!(decoded.payload_bytes(), [value]),
        }
    }
}
