//! DX Film Edge — the barcode along the edge of 35mm film.
//!
//! Validation:
//! - The data-track bit layout and clock track are taken from the **zxing-cpp**
//!   reference reader `core/src/oned/ODDXFilmEdgeReader.cpp` (constants
//!   `CLOCK_PATTERN_FN`, `DATA_LENGTH_FN`, the DX1/DX2/frame/parity field offsets and
//!   the example `115-10/11A`).
//! - The DX-number bit weighting (part 1 = 7 bits weighted 64…1, part 2 = 4 bits
//!   weighted 8…1) matches the **Wikipedia "DX encoding"** article's documented stripe
//!   weighting.
//! - encode → decode → re-encode is byte-identical across the DX/frame ranges.

use anyd::codes::dxfilm::{DxFilmDecoder, DxFilmEncoder, DxFilmMeta};
use anyd::output::{BitMatrix, Encoding};
use anyd::symbol::SymbolMeta;
use anyd::symbology::Symbology;
use anyd::traits::{Decode, Encode};

fn matrix_of(symbol: &anyd::symbol::Symbol) -> BitMatrix {
    match DxFilmEncoder::new().encode(symbol).unwrap() {
        Encoding::Matrix(m) => m,
        Encoding::Linear(_) => panic!("DX Film Edge must render as a matrix"),
    }
}

/// The data-track bit at logical index `i` (dark = 1), i.e. row 1, module `5 + i`.
fn data_bit(m: &BitMatrix, i: usize) -> u8 {
    m.get(5 + i, 1) as u8
}

fn bits_to_uint(m: &BitMatrix, start: usize, len: usize) -> u32 {
    (0..len).fold(0u32, |acc, i| (acc << 1) | data_bit(m, start + i) as u32)
}

/// Expected clock track for a symbol `width` modules wide: a 5-wide bar, then evenly
/// spaced single modules, then a 3-wide bar (zxing-cpp `CLOCK_PATTERN_*`).
fn expected_clock(width: usize) -> Vec<bool> {
    let mut row = vec![false; width];
    let mut x = 0usize;
    let mut dark = true;
    let mut runs = vec![5usize];
    runs.extend(std::iter::repeat_n(1, width - 8));
    runs.push(3);
    for run in runs {
        if dark {
            for m in row.iter_mut().skip(x).take(run) {
                *m = true;
            }
        }
        x += run;
        dark = !dark;
    }
    row
}

#[test]
fn dx_number_bit_weighting_is_documented() {
    // Wikipedia: part 1 occupies 7 bits (weights 64..1), part 2 occupies 4 bits
    // (weights 8..1), most-significant first. 115 = 0b1110011, 10 = 0b1010.
    let enc = DxFilmEncoder::new();
    let symbol = enc.build(115, 10, None).unwrap();
    let m = matrix_of(&symbol);

    // Older (no frame number) variant: 5 + 15 + 3 = 23 modules, two rows.
    assert_eq!(m.width(), 23);
    assert_eq!(m.height(), 2);

    // DX part 1 in data bits 1..=7, DX part 2 in bits 9..=12.
    assert_eq!(bits_to_uint(&m, 1, 7), 115);
    assert_eq!(bits_to_uint(&m, 9, 4), 10);

    // Separators (always light) at bits 0, 8, and the trailing separator at 14.
    assert_eq!(data_bit(&m, 0), 0);
    assert_eq!(data_bit(&m, 8), 0);
    assert_eq!(data_bit(&m, 14), 0);
}

#[test]
fn start_stop_and_clock_patterns() {
    let enc = DxFilmEncoder::new();
    let m = matrix_of(&enc.build_text("115-10/11A").unwrap());

    // Frame-number variant: 5 + 23 + 3 = 31 modules.
    assert_eq!(m.width(), 31);

    // Data-track start pattern: bar space bar space bar.
    for (x, expect) in [(0, true), (1, false), (2, true), (3, false), (4, true)] {
        assert_eq!(m.get(x, 1), expect, "start module {x}");
    }
    // Data-track stop pattern: bar space bar, right after the 23 data bits.
    let stop = 5 + 23;
    assert!(m.get(stop, 1));
    assert!(!m.get(stop + 1, 1));
    assert!(m.get(stop + 2, 1));

    // Clock track (row 0) matches the reference pattern.
    let clock = expected_clock(31);
    for (x, &c) in clock.iter().enumerate() {
        assert_eq!(m.get(x, 0), c, "clock module {x}");
    }
}

#[test]
fn frame_number_fields() {
    // "115-10/11A": DX1=115, DX2=10, frame=11, half-frame flag set.
    let enc = DxFilmEncoder::new();
    let m = matrix_of(&enc.build_text("115-10/11A").unwrap());
    assert_eq!(bits_to_uint(&m, 1, 7), 115);
    assert_eq!(bits_to_uint(&m, 9, 4), 10);
    assert_eq!(bits_to_uint(&m, 13, 6), 11); // frame number
    assert_eq!(data_bit(&m, 19), 1); // half-frame "A" flag
}

#[test]
fn even_parity_holds() {
    // The parity bit makes the number of set bits (up to and including parity) even.
    let enc = DxFilmEncoder::new();
    for text in ["115-10", "1-0", "127-15", "42-7/33A", "96-0/0"] {
        let m = matrix_of(&enc.build_text(text).unwrap());
        let data_len = m.width() - 8;
        let ones = (0..data_len - 1).filter(|&i| data_bit(&m, i) == 1).count();
        assert_eq!(ones % 2, 0, "parity for {text}");
    }
}

#[test]
fn roundtrip_without_frame() {
    let enc = DxFilmEncoder::new();
    let dec = DxFilmDecoder::new();
    for part1 in [1u8, 7, 64, 115, 127] {
        for part2 in [0u8, 1, 10, 15] {
            let symbol = enc.build(part1, part2, None).unwrap();
            let encoding = enc.encode(&symbol).unwrap();
            let decoded = dec.decode(&encoding).unwrap();
            assert_eq!(decoded.symbology, Symbology::DxFilmEdge);
            assert_eq!(decoded.text(), symbol.text());
            assert_eq!(decoded.meta, symbol.meta);
            assert_eq!(
                decoded.meta,
                SymbolMeta::DxFilm(DxFilmMeta {
                    has_frame_number: false
                })
            );
            assert_eq!(enc.encode(&decoded).unwrap(), encoding);
        }
    }
}

#[test]
fn roundtrip_with_frame() {
    let enc = DxFilmEncoder::new();
    let dec = DxFilmDecoder::new();
    for &(part1, part2, frame, half) in &[
        (115u8, 10u8, 11u8, true),
        (1, 0, 0, false),
        (127, 15, 63, true),
        (42, 7, 33, false),
    ] {
        let symbol = enc.build(part1, part2, Some((frame, half))).unwrap();
        let encoding = enc.encode(&symbol).unwrap();
        let decoded = dec.decode(&encoding).unwrap();
        assert_eq!(decoded.text(), symbol.text());
        assert_eq!(
            decoded.meta,
            SymbolMeta::DxFilm(DxFilmMeta {
                has_frame_number: true
            })
        );
        assert_eq!(enc.encode(&decoded).unwrap(), encoding);

        let expected = format!("{part1}-{part2}/{frame}{}", if half { "A" } else { "" });
        assert_eq!(decoded.text().as_deref(), Some(expected.as_str()));
    }
}

#[test]
fn text_builder_roundtrip() {
    let enc = DxFilmEncoder::new();
    for text in ["115-10", "1-0", "127-15", "115-10/11A", "96-0/0", "42-7/33"] {
        let symbol = enc.build_text(text).unwrap();
        assert_eq!(symbol.text().as_deref(), Some(text));
        let decoded = DxFilmDecoder::new()
            .decode(&enc.encode(&symbol).unwrap())
            .unwrap();
        assert_eq!(decoded.text().as_deref(), Some(text));
    }
}

#[test]
fn rejects_out_of_range() {
    let enc = DxFilmEncoder::new();
    assert!(enc.build(0, 0, None).is_err()); // part1 must be >= 1
    assert!(enc.build(128, 0, None).is_err()); // part1 max 127
    assert!(enc.build(1, 16, None).is_err()); // part2 max 15
    assert!(enc.build(1, 0, Some((64, false))).is_err()); // frame max 63
    assert!(enc.build_text("not-a-code").is_err());
    assert!(enc.build_text("115").is_err()); // missing '-'
}
