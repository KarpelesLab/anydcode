//! End-to-end Aztec round-trip tests: encode → decode → re-encode must be identical,
//! across encodation modes and both compact and full-range sizes, and must survive
//! correctable errors.
#![cfg(all(feature = "decode", feature = "encode", feature = "aztec"))]

use anyd::codes::aztec::{AztecDecoder, AztecEncoder};
use anyd::output::Encoding;
use anyd::segment::Segment;
use anyd::symbol::SymbolMeta;
use anyd::traits::{Decode, Encode};

/// Assert a payload decodes to the same bytes/meta and re-encodes identically.
fn assert_lossless(payload: &[u8]) {
    let enc = AztecEncoder::new();
    let symbol = enc.build(vec![Segment::byte(payload.to_vec())]).unwrap();
    let encoding = enc.encode(&symbol).unwrap();

    let decoded = AztecDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.payload_bytes(),
        payload,
        "payload differs after decode"
    );
    assert_eq!(decoded.meta, symbol.meta, "meta differs after decode");

    let reencoded = enc.encode(&decoded).unwrap();
    assert_eq!(reencoded, encoding, "re-encoding is not byte-identical");
}

#[test]
fn upper_and_digit_modes() {
    assert_lossless(b"AZTEC CODE 2026");
}

#[test]
fn lower_mixed_punct_modes() {
    assert_lossless(b"Hello, World! (mixed & punct: a-z/0-9)");
}

#[test]
fn binary_mode_high_bytes() {
    assert_lossless(&[0x00, 0x01, 0x80, 0xff, 0xfe, b'A', 0x0e, 0x1a, 0x7f]);
}

#[test]
fn utf8_payload() {
    assert_lossless("héllo wörld — 日本語".as_bytes());
}

#[test]
fn all_control_and_symbols() {
    assert_lossless(b"a\tb\nc\rd @\\^_`|~ #$%&*+=<>?[]{}");
}

#[test]
fn compact_sizes_scale_with_length() {
    // Increasing payloads walk through the compact layer counts.
    let mut compact_layers_seen = std::collections::BTreeSet::new();
    for n in [1usize, 10, 30, 60, 120, 200] {
        let payload = vec![b'A'; n];
        let enc = AztecEncoder::new();
        let symbol = enc.build(vec![Segment::byte(payload.clone())]).unwrap();
        if let SymbolMeta::Aztec(m) = &symbol.meta
            && m.compact
        {
            compact_layers_seen.insert(m.layers);
        }
        let encoding = enc.encode(&symbol).unwrap();
        let decoded = AztecDecoder::new().decode(&encoding).unwrap();
        assert_eq!(decoded.payload_bytes(), payload);
        assert_eq!(enc.encode(&decoded).unwrap(), encoding);
    }
    // We should have exercised more than one compact size.
    assert!(
        compact_layers_seen.len() >= 2,
        "expected multiple compact sizes"
    );
}

#[test]
fn full_range_symbol() {
    // A payload too large for compact (max 4 layers) forces a full-range symbol.
    let payload: Vec<u8> = (0..600u16).map(|i| b'A' + (i % 26) as u8).collect();
    let enc = AztecEncoder::new();
    let symbol = enc.build(vec![Segment::byte(payload.clone())]).unwrap();
    match &symbol.meta {
        SymbolMeta::Aztec(m) => assert!(!m.compact, "expected a full-range symbol"),
        _ => panic!("expected Aztec meta"),
    }
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = AztecDecoder::new().decode(&encoding).unwrap();
    assert_eq!(decoded.payload_bytes(), payload);
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

#[test]
fn survives_correctable_errors() {
    // Corrupt a few data modules and confirm Reed–Solomon still recovers the payload.
    let enc = AztecEncoder::new();
    let symbol = enc.build_text("RESILIENT AZTEC PAYLOAD").unwrap();
    let Encoding::Matrix(mut matrix) = enc.encode(&symbol).unwrap() else {
        panic!("expected a matrix");
    };
    // Flip a short run of modules away from the bullseye.
    for k in 0..4 {
        let x = matrix.width() - 2 - k;
        let y = 1 + k;
        let v = matrix.get(x, y);
        matrix.set(x, y, !v);
    }
    let decoded = AztecDecoder::new()
        .decode(&Encoding::Matrix(matrix))
        .unwrap();
    assert_eq!(decoded.text().as_deref(), Some("RESILIENT AZTEC PAYLOAD"));
}

#[test]
fn text_convenience_roundtrip() {
    let enc = AztecEncoder::new();
    let symbol = enc
        .build_text("https://github.com/KarpelesLab/anydcode")
        .unwrap();
    let encoding = enc.encode(&symbol).unwrap();
    let decoded = AztecDecoder::new().decode(&encoding).unwrap();
    assert_eq!(
        decoded.text().as_deref(),
        Some("https://github.com/KarpelesLab/anydcode")
    );
    assert_eq!(enc.encode(&decoded).unwrap(), encoding);
}

// ---- Third-party reference vectors -------------------------------------------
//
// Module matrices produced by zint 2.16 (`zint -b AZTEC --dump`), one string per row,
// '1' = dark. They pin everything a self round-trip cannot: the orientation marks,
// the reference grid, the mode message and the spiral data path.

/// Parse a row-string vector into a matrix with the Aztec quiet zone.
fn matrix_from_rows(rows: &[&str]) -> anyd::output::BitMatrix {
    let mut m = anyd::output::BitMatrix::new(rows[0].len(), rows.len(), 0);
    for (y, row) in rows.iter().enumerate() {
        for (x, c) in row.bytes().enumerate() {
            m.set(x, y, c == b'1');
        }
    }
    m
}

/// `zint -b AZTEC --vers=1 -d HELLO`: a compact 1-layer symbol.
#[rustfmt::skip]
const ZINT_HELLO_COMPACT: &[&str] = &[
    "001100100101001",
    "010111011010000",
    "001100000100101",
    "101111111111111",
    "010100000001010",
    "100101111101101",
    "011101000101010",
    "100101010101110",
    "100101000101010",
    "110101111101011",
    "010100000001011",
    "100111111111110",
    "000011101110010",
    "111110110011110",
    "011010101010111",
];

/// `zint -b AZTEC --vers=9 -d "AZTEC FULL RANGE SYMBOL WITH REFERENCE GRID"`: a
/// full-range 5-layer symbol, the smallest with off-centre reference-grid lines.
#[rustfmt::skip]
const ZINT_FULL_5_LAYERS: &[&str] = &[
    "0010001100110001101101101101110101110",
    "0101110100011111000110111000001000000",
    "1010101010101010101010101010101010101",
    "0100111111110001010110000110101001000",
    "1011011001100111011101100011110001101",
    "1101010110010110010110111100000101010",
    "1011011110100010111111110000101011100",
    "1000000111000100100011101100101101001",
    "1010010101000010111001101011111001100",
    "0100001010100111010100000110000000011",
    "1010110011100000011001111111011111111",
    "0000110011011001000000000101000011000",
    "1011000100111111111111111101110110111",
    "0000101111001000000000001011001110000",
    "0010110100011011111111101110111000111",
    "0100011101111010000000101110011111000",
    "0011111111101010111110101010110100110",
    "1101110100101010100010101101011000011",
    "1010101010101010101010101010101010101",
    "1100101000111010100010101011001011000",
    "0110010101001010111110101011100010111",
    "1001111101111010000000101110011010010",
    "0111001000111011111111101101110101100",
    "1001001011111000000000001010110101011",
    "1010001011101111111111111110001010100",
    "1101010110000000100110010000111010011",
    "0110001100111001001100100001100110100",
    "0000101000110001110001110101110011011",
    "0011101000001011001010000011100011100",
    "1100010000101000100111111110100000000",
    "0011101101111111111001000000101001110",
    "1100110110101101010001101101001110001",
    "0011111101000000011111110010100000101",
    "0100010011111111000111101110000101001",
    "1010101010101010101010101010101010101",
    "0101100100011011010001100100011000001",
    "0111000010010010011100100011001001101",
];

/// A third-party symbol must decode, and re-encoding the decoded symbol must
/// reproduce the third-party matrix module for module.
fn assert_matches_reference(rows: &[&str], payload: &[u8], compact: bool, layers: u8) {
    let reference = Encoding::Matrix(matrix_from_rows(rows));
    let decoded = AztecDecoder::new().decode(&reference).unwrap();
    assert_eq!(decoded.payload_bytes(), payload);
    let SymbolMeta::Aztec(meta) = &decoded.meta else {
        panic!("expected AztecMeta");
    };
    assert_eq!((meta.compact, meta.layers), (compact, layers));
    assert_eq!(AztecEncoder::new().encode(&decoded).unwrap(), reference);
}

#[test]
fn matches_zint_compact_symbol() {
    assert_matches_reference(ZINT_HELLO_COMPACT, b"HELLO", true, 1);
}

#[test]
fn matches_zint_full_symbol_with_reference_grid() {
    assert_matches_reference(
        ZINT_FULL_5_LAYERS,
        b"AZTEC FULL RANGE SYMBOL WITH REFERENCE GRID",
        false,
        5,
    );
}

#[test]
fn orientation_marks_follow_iso_24778() {
    // Three dark modules at the top-left corner of the mode-message ring, two at the
    // top-right, one at the bottom-right, none at the bottom-left.
    let enc = AztecEncoder::new();
    for payload in [&b"A"[..], &[b'x'; 300][..]] {
        let symbol = enc.build(vec![Segment::byte(payload.to_vec())]).unwrap();
        let SymbolMeta::Aztec(meta) = &symbol.meta else {
            panic!("expected AztecMeta");
        };
        let r = if meta.compact { 5 } else { 7 };
        let Encoding::Matrix(m) = enc.encode(&symbol).unwrap() else {
            panic!("expected a matrix");
        };
        let c = m.width() / 2;
        let (lo, hi) = (c - r, c + r);
        let marks = [
            (lo, lo, true),
            (lo + 1, lo, true),
            (lo, lo + 1, true),
            (hi, lo, true),
            (hi, lo + 1, true),
            (hi - 1, lo, false),
            (hi, hi, false),
            (hi, hi - 1, true),
            (hi - 1, hi, false),
            (lo, hi, false),
            (lo + 1, hi, false),
            (lo, hi - 1, false),
        ];
        for (x, y, dark) in marks {
            assert_eq!(m.get(x, y), dark, "orientation module ({x},{y})");
        }
    }
}

/// `zint -b AZTEC -d ". , : \r\n.,:"`: zint latches into Punct (M/L, P/L) and uses the
/// two-byte Punct codes, which a shift-only decoder cannot follow.
#[rustfmt::skip]
const ZINT_PUNCT_LATCH: &[&str] = &[
    "000101011001000",
    "110110100001011",
    "101100000110100",
    "111111111111111",
    "111100000001101",
    "101101111101001",
    "001101000101100",
    "010101010101001",
    "100101000101001",
    "011101111101111",
    "001100000001111",
    "000111111111111",
    "100011000110000",
    "000011010011100",
    "101101001111001",
];

/// `zint -b AZTEC --eci=26 -d "héllo wörld"` (UTF-8): starts with an FLG(2) ECI escape.
#[rustfmt::skip]
const ZINT_ECI_26: &[&str] = &[
    "0011001001101001101",
    "0001010110011011011",
    "0111100100111000110",
    "0010011101110000000",
    "0001110101010011101",
    "0110111111111110110",
    "0110110000000100010",
    "0010010111110100100",
    "1011110100010110111",
    "0100010101010111111",
    "0001110100010101110",
    "0101110111110111100",
    "1110110000000100111",
    "0000011111111111010",
    "0110001111001001010",
    "0011010011111010000",
    "1111110000000011100",
    "1101100111100111000",
    "1100010010001100111",
];

#[test]
fn decodes_third_party_punct_latch() {
    let reference = Encoding::Matrix(matrix_from_rows(ZINT_PUNCT_LATCH));
    let decoded = AztecDecoder::new().decode(&reference).unwrap();
    assert_eq!(decoded.payload_bytes(), b". , : \r\n.,:");
}

#[test]
fn decodes_third_party_eci_escape() {
    let reference = Encoding::Matrix(matrix_from_rows(ZINT_ECI_26));
    let decoded = AztecDecoder::new().decode(&reference).unwrap();
    assert_eq!(
        decoded.modes(),
        vec![anyd::segment::Mode::Eci(26), anyd::segment::Mode::Byte]
    );
    assert_eq!(decoded.payload_bytes(), "héllo wörld".as_bytes());
}

#[test]
fn unsupported_layer_counts_are_errors_not_panics() {
    // Hand-built metadata outside the supported sizes (compact 1-4, full 1-22 layers)
    // must be rejected cleanly, whatever the payload length.
    let enc = AztecEncoder::new();
    for (compact, layers) in [
        (true, 0u8),
        (true, 5),
        (true, 23),
        (false, 0),
        (false, 23),
        (false, 32),
        (false, 255),
    ] {
        for len in [1usize, 40, 1500] {
            let symbol = anyd::symbol::Symbol::new(
                anyd::symbology::Symbology::Aztec,
                vec![Segment::byte(vec![b'A'; len])],
                SymbolMeta::Aztec(anyd::codes::aztec::AztecMeta {
                    compact,
                    layers,
                    rune: false,
                }),
            );
            assert!(
                enc.encode(&symbol).is_err(),
                "compact={compact} layers={layers} len={len}"
            );
        }
    }
}

/// Deterministic xorshift64* generator for the no-panic fuzz loops.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn decoder_never_panics_on_garbage() {
    let mut rng = Rng(0x00A2_7EC0_DE5E_ED01);
    let dec = AztecDecoder::new();
    // Every supported size (compact, full, rune) plus a few invalid ones.
    let mut sizes: Vec<usize> = vec![0, 1, 10, 11, 12, 14, 16, 152];
    sizes.extend((1..=4).map(|l| 11 + 4 * l));
    sizes.extend((1..=32).map(|l| {
        let base = 14 + 4 * l;
        base + 1 + 2 * ((base / 2 - 1) / 15)
    }));
    for &size in &sizes {
        for density in [0u64, 1, 2, 3, 4] {
            let mut m = anyd::output::BitMatrix::new(size, size, 0);
            for y in 0..size {
                for x in 0..size {
                    m.set(x, y, rng.next() % 4 < density);
                }
            }
            let _ = dec.decode(&Encoding::Matrix(m));
        }
    }
    // Non-square grids are rejected.
    let m = anyd::output::BitMatrix::new(15, 19, 0);
    assert!(dec.decode(&Encoding::Matrix(m)).is_err());
}

#[test]
fn corrupted_symbols_never_panic_or_misdecode_lightly_damaged() {
    let mut rng = Rng(0x5EED_0000_0000_A27E);
    let enc = AztecEncoder::new();
    let dec = AztecDecoder::new();
    for len in [1usize, 7, 20, 45, 90, 200, 600] {
        let payload: Vec<u8> = (0..len).map(|_| rng.below(256) as u8).collect();
        let symbol = enc.build(vec![Segment::byte(payload.clone())]).unwrap();
        let Encoding::Matrix(clean) = enc.encode(&symbol).unwrap() else {
            panic!("expected a matrix");
        };
        let size = clean.width();
        for flips in [1usize, 1, 1, 2, 8, 40, 400] {
            let mut m = clean.clone();
            for _ in 0..flips {
                let (x, y) = (rng.below(size), rng.below(size));
                let v = m.get(x, y);
                m.set(x, y, !v);
            }
            let result = dec.decode(&Encoding::Matrix(m));
            // One flipped module damages at most one codeword (or one mode-message
            // word); every size the encoder picks carries at least two check words.
            if flips == 1 {
                assert_eq!(result.unwrap().payload_bytes(), payload, "len {len}");
            }
        }
    }
}

#[test]
fn every_length_up_to_capacity_round_trips() {
    // Sweep payload lengths across every size boundary until the encoder reports a
    // capacity error: each accepted length must round-trip, and once a length is
    // refused every longer one must be too (no panic, no silent truncation).
    let enc = AztecEncoder::new();
    let dec = AztecDecoder::new();
    let fills: [fn(usize) -> u8; 3] = [
        |_| b'A',
        |i| 0x80 | (i as u8 & 0x7f),
        |i| b"Az 09,.!\x01\xff"[i % 10],
    ];
    for fill in fills {
        let mut refused = false;
        let mut len = 1usize;
        while len < 2600 {
            let payload: Vec<u8> = (0..len).map(fill).collect();
            match enc.build(vec![Segment::byte(payload.clone())]) {
                Ok(symbol) => {
                    assert!(!refused, "length {len} accepted after a shorter refusal");
                    let encoding = enc.encode(&symbol).unwrap();
                    let decoded = dec.decode(&encoding).unwrap();
                    assert_eq!(decoded.payload_bytes(), payload, "length {len}");
                    assert_eq!(decoded.meta, symbol.meta, "length {len}");
                }
                Err(_) => refused = true,
            }
            // Dense near the binary-shift length boundaries, sparser beyond.
            len += if len < 80 { 1 } else { 37 };
        }
        assert!(refused, "capacity limit never reached");
    }
}
